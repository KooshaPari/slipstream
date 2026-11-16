// Durable storage for this client's persistent identity, with the failure policy that
// matters more than the storage itself: a stored-but-unreadable identity is an ERROR, never
// a cue to mint a replacement. Minting a new key silently is what turns one confused launch
// into a stream of phantom "This device" pairing requests on every host the Mac has paired
// with.
//
// `ClientIdentityFileStore` is the durable primary (Application Support, 0600, atomic write);
// `ClientIdentityResolver` composes it with any best-effort secondary backing (the Keychain,
// which an ad-hoc-signed build cannot rely on: no keychain-access-groups entitlement, and a
// per-binary ACL that re-prompts whenever the signature changes).

import Foundation

/// Identity persistence failures. All of them are fatal to the current connect attempt by
/// design — the caller must surface them rather than fall back to an anonymous identity.
public enum IdentityStoreError: Error, Equatable {
    /// A stored identity exists but cannot be decoded (truncated, empty fields, not our JSON).
    case corrupt(String)
    /// A stored identity exists but the file could not be read (permissions, IO).
    case unreadable(String, String)
    /// The identity could not be written to durable storage, so returning it would create
    /// a pairing that evaporates on relaunch.
    case notPersisted(String, String)
}

/// One place an identity can live. `loadIdentity()` returns nil only when the backing has
/// never stored one; it throws when something IS there but cannot be used.
public protocol ClientIdentityBacking: Sendable {
    /// Short, log-friendly location description ("file", "keychain").
    var name: String { get }
    func loadIdentity() throws -> ClientIdentity?
    func saveIdentity(_ identity: ClientIdentity) throws
}

/// The durable primary: a single JSON file in a caller-chosen directory.
///
/// Nothing here is per-PID or temporary — the directory is stable across launches, the file
/// is written atomically (so a crash mid-write cannot truncate a working identity) and both
/// the directory (0700) and the file (0600) are owner-only, since the file holds a private key.
public struct ClientIdentityFileStore: ClientIdentityBacking {
    public let name: String
    public let fileURL: URL

    public init(directory: URL, name: String = "file") {
        self.name = name
        self.fileURL = directory.appendingPathComponent("client-identity.json")
    }

    private struct Stored: Codable {
        var certPEM: String
        var keyPEM: String
    }

    public func loadIdentity() throws -> ClientIdentity? {
        let fm = FileManager.default
        var isDir: ObjCBool = false
        guard fm.fileExists(atPath: fileURL.path, isDirectory: &isDir), !isDir.boolValue else {
            return nil
        }
        let data: Data
        do {
            data = try Data(contentsOf: fileURL)
        } catch {
            // Exists but unreadable: possibly an identity we must not shadow.
            throw IdentityStoreError.unreadable(fileURL.path, error.localizedDescription)
        }
        let stored: Stored
        do {
            stored = try JSONDecoder().decode(Stored.self, from: data)
        } catch {
            throw IdentityStoreError.corrupt(fileURL.path)
        }
        guard !stored.certPEM.isEmpty, !stored.keyPEM.isEmpty else {
            throw IdentityStoreError.corrupt(fileURL.path)
        }
        return ClientIdentity(certPEM: stored.certPEM, keyPEM: stored.keyPEM)
    }

    public func saveIdentity(_ identity: ClientIdentity) throws {
        let fm = FileManager.default
        let dir = fileURL.deletingLastPathComponent()
        do {
            try fm.createDirectory(
                at: dir, withIntermediateDirectories: true,
                attributes: [.posixPermissions: 0o700])
            let data = try JSONEncoder().encode(
                Stored(certPEM: identity.certPEM, keyPEM: identity.keyPEM))
            guard !data.isEmpty else {
                throw IdentityStoreError.notPersisted(fileURL.path, "empty encode")
            }
            try data.write(to: fileURL, options: [.atomic])
            try fm.setAttributes([.posixPermissions: 0o600], ofItemAtPath: fileURL.path)
        } catch let error as IdentityStoreError {
            throw error
        } catch {
            throw IdentityStoreError.notPersisted(fileURL.path, error.localizedDescription)
        }
    }
}

/// Loads the identity this client presents, minting one only when NO backing holds one.
///
/// The first backing is authoritative and strict: if it holds an identity that cannot be
/// decoded it throws, and the caller surfaces a clear error. Later backings are best-effort
/// caches (the Keychain on macOS can legitimately refuse an ad-hoc-signed binary, and that
/// refusal must not brick a Mac whose durable file is intact); a hit there is copied into the
/// primary so the next launch does not depend on it again.
public struct ClientIdentityResolver: Sendable {
    public let backings: [any ClientIdentityBacking]

    public init(backings: [any ClientIdentityBacking]) {
        precondition(!backings.isEmpty, "at least one identity backing is required")
        self.backings = backings
    }

    public func load() throws -> ClientIdentity {
        let primary = backings[0]
        if let identity = try primary.loadIdentity() {
            return identity
        }
        for backing in backings.dropFirst() {
            if let identity = try? backing.loadIdentity() {
                try? primary.saveIdentity(identity) // heal the durable copy
                return identity
            }
        }
        let fresh = try generateIdentity()
        do {
            try primary.saveIdentity(fresh)
        } catch {
            // Fail closed: an in-memory identity would produce a pairing the host trusts and
            // this Mac forgets — exactly the phantom-device loop this store exists to prevent.
            throw IdentityStoreError.notPersisted(primary.name, "\(error)")
        }
        for backing in backings.dropFirst() {
            try? backing.saveIdentity(fresh)
        }
        return fresh
    }
}
