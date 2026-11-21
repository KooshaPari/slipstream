// This client's persistent slipstream/1 identity: a self-signed certificate + key (PEM),
// minted once and then persisted so every launch and every session presents the same
// certificate fingerprint — that fingerprint is how a paired host recognises this Mac.
//
// Durable home: `~/Library/Application Support/Slipstream/client-identity.json` (0600), via
// `ClientIdentityFileStore`. The Keychain is a best-effort secondary: an ad-hoc-signed build
// has no `keychain-access-groups` entitlement, so the data-protection keychain returns
// errSecMissingEntitlement and the legacy file keychain is gated by a per-binary ACL that
// changes with every resign. That is exactly why a launch used to present a new identity:
// the write quietly failed, `add` downgraded to an in-memory identity, and the callers'
// `try?` turned a Keychain refusal into an anonymous connect. Both paths now fail closed:
// an unreadable stored identity is an error the UI surfaces, never a reason to mint.

import Foundation
import SlipstreamKit
import Security
import os

final class ClientIdentityStore: @unchecked Sendable {
    static let shared = ClientIdentityStore()

    private let log = Logger(subsystem: "io.slipstream", category: "identity")
    private let lock = NSLock()
    private var cached: ClientIdentity?
    private let resolver: ClientIdentityResolver

    init(resolver: ClientIdentityResolver = ClientIdentityStore.defaultResolver()) {
        self.resolver = resolver
    }

    /// `~/Library/Application Support/Slipstream` — stable across launches and rebuilds,
    /// unlike the code-signature-scoped Keychain ACL.
    static func defaultDirectory() -> URL {
        let base = FileManager.default.urls(
            for: .applicationSupportDirectory, in: .userDomainMask).first
            ?? URL(fileURLWithPath: NSHomeDirectory())
                .appendingPathComponent("Library/Application Support", isDirectory: true)
        return base.appendingPathComponent("Slipstream", isDirectory: true)
    }

    static func defaultResolver() -> ClientIdentityResolver {
        ClientIdentityResolver(backings: [
            ClientIdentityFileStore(directory: defaultDirectory()),
            KeychainIdentityBacking(),
        ])
    }

    /// The identity to present when connecting. Throws `IdentityStoreError` when a stored
    /// identity exists but cannot be used, or when a freshly minted one cannot be persisted —
    /// callers must surface that, not fall back to an anonymous connect.
    /// Blocking (file/Keychain IO + key generation) — call off the main actor.
    func load() throws -> ClientIdentity {
        lock.lock()
        defer { lock.unlock() }
        if let cached { return cached }
        do {
            let identity = try resolver.load()
            cached = identity
            return identity
        } catch {
            log.error("client identity unavailable: \(String(describing: error), privacy: .public)")
            throw error
        }
    }

    /// Pairing uses the same durable path: `load()` only ever returns an identity that is
    /// already on disk (or throws), so a pairing can no longer outlive its client key.
    func loadForPairing() throws -> ClientIdentity {
        try load()
    }
}

/// Best-effort Keychain cache. Never authoritative: its read refusal is ignored by
/// `ClientIdentityResolver` while the durable file is intact.
struct KeychainIdentityBacking: ClientIdentityBacking {
    let name = "keychain"
    private let log = Logger(subsystem: "io.slipstream", category: "identity")

    private struct Stored: Codable {
        var certPEM: String
        var keyPEM: String
    }

    private enum ReadResult {
        case found(ClientIdentity)
        case absent
        case corrupt
        case denied(OSStatus)
    }

    private static func query(dataProtection: Bool) -> [String: Any] {
        var q: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: "io.slipstream",
            kSecAttrAccount as String: "client-identity",
        ]
        if dataProtection { q[kSecUseDataProtectionKeychain as String] = true }
        return q
    }

    func loadIdentity() throws -> ClientIdentity? {
        var result = read(dataProtection: true)
        if case .denied(errSecMissingEntitlement) = result {
            result = read(dataProtection: false)
        }
        switch result {
        case .found(let identity):
            log.info("keychain identity loaded")
            return identity
        case .absent:
            return nil
        case .corrupt:
            log.error("keychain identity corrupt")
            throw IdentityStoreError.corrupt("keychain")
        case .denied(let status):
            log.error("keychain identity read refused status=\(status)")
            throw IdentityStoreError.unreadable("keychain", "OSStatus \(status)")
        }
    }

    func saveIdentity(_ identity: ClientIdentity) throws {
        guard let data = try? JSONEncoder().encode(
            Stored(certPEM: identity.certPEM, keyPEM: identity.keyPEM))
        else { throw IdentityStoreError.notPersisted(name, "encode failed") }

        var add = Self.query(dataProtection: true)
        add[kSecValueData as String] = data
        add[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlock
        var status = SecItemAdd(add as CFDictionary, nil)
        if status == errSecMissingEntitlement {
            var legacy = Self.query(dataProtection: false)
            legacy[kSecValueData as String] = data
            status = SecItemAdd(legacy as CFDictionary, nil)
        }
        log.info("keychain identity save status=\(status)")
        guard status == errSecSuccess || status == errSecDuplicateItem else {
            throw IdentityStoreError.notPersisted(name, "OSStatus \(status)")
        }
    }

    private func read(dataProtection: Bool) -> ReadResult {
        var query = Self.query(dataProtection: dataProtection)
        query[kSecReturnData as String] = true
        var out: CFTypeRef?
        switch SecItemCopyMatching(query as CFDictionary, &out) {
        case errSecSuccess:
            guard let data = out as? Data,
                  let stored = try? JSONDecoder().decode(Stored.self, from: data),
                  !stored.certPEM.isEmpty, !stored.keyPEM.isEmpty
            else { return .corrupt }
            return .found(ClientIdentity(certPEM: stored.certPEM, keyPEM: stored.keyPEM))
        case errSecItemNotFound:
            return .absent
        case let status:
            return .denied(status)
        }
    }
}
