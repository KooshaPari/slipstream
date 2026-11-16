// Durable client-identity storage: a relaunch must present the SAME identity (otherwise every
// launch looks like a new device to a paired host), and a stored-but-unreadable identity must
// surface an error rather than minting a replacement.

import XCTest
@testable import SlipstreamKit

final class IdentityVaultTests: XCTestCase {
    private var dir: URL!

    override func setUpWithError() throws {
        dir = FileManager.default.temporaryDirectory
            .appendingPathComponent("slipstream-identity-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
    }

    override func tearDownWithError() throws {
        if let dir { try? FileManager.default.removeItem(at: dir) }
    }

    // MARK: helpers

    /// A backing that only fails on demand, to model a Keychain that refuses this build.
    private struct RefusingBacking: ClientIdentityBacking {
        let name = "refusing"
        func loadIdentity() throws -> ClientIdentity? {
            throw IdentityStoreError.unreadable(name, "OSStatus -25308")
        }
        func saveIdentity(_ identity: ClientIdentity) throws {
            throw IdentityStoreError.notPersisted(name, "OSStatus -25293")
        }
    }

    private struct RecordingBacking: ClientIdentityBacking, @unchecked Sendable {
        let name: String
        final class Box: @unchecked Sendable {
            var saved: [ClientIdentity] = []
        }
        let box = Box()
        func loadIdentity() throws -> ClientIdentity? { nil }
        func saveIdentity(_ identity: ClientIdentity) throws { box.saved.append(identity) }
    }

    // MARK: relaunch stability

    func testSecondLaunchReusesSameIdentityFromSameBackingStore() throws {
        // Launch 1: nothing stored, so the resolver mints and persists.
        let firstLaunch = ClientIdentityResolver(backings: [
            ClientIdentityFileStore(directory: dir),
        ])
        let first = try firstLaunch.load()
        XCTAssertTrue(first.certPEM.contains("BEGIN CERTIFICATE"))

        // Launch 2: a FRESH store instance over the same backing store must return the
        // identity launch 1 persisted — this is the defect that registered a new "This
        // device" on every launch.
        let secondLaunch = ClientIdentityResolver(backings: [
            ClientIdentityFileStore(directory: dir),
        ])
        let second = try secondLaunch.load()

        XCTAssertEqual(second.certPEM, first.certPEM)
        XCTAssertEqual(second.keyPEM, first.keyPEM)

        // And a third launch, to show it is stable rather than a two-launch coincidence.
        let third = try ClientIdentityResolver(backings: [ClientIdentityFileStore(directory: dir)])
            .load()
        XCTAssertEqual(third.certPEM, first.certPEM)
    }

    func testIdentityFileIsOwnerOnly() throws {
        _ = try ClientIdentityResolver(backings: [ClientIdentityFileStore(directory: dir)]).load()
        let file = dir.appendingPathComponent("client-identity.json")
        let attrs = try FileManager.default.attributesOfItem(atPath: file.path)
        let perms = attrs[.posixPermissions] as? NSNumber
        XCTAssertEqual(perms?.intValue, 0o600, "identity file holds a private key: owner-only")
    }

    func testKeychainRefusalDoesNotBlockDurableFile() throws {
        // The shipped bug: a Keychain that refuses an ad-hoc-signed build was treated as
        // fatal at one layer and as "mint a new one" at another. Now it is a best-effort
        // secondary — the durable file still wins.
        let resolver = ClientIdentityResolver(backings: [
            ClientIdentityFileStore(directory: dir),
            RefusingBacking(),
        ])
        let first = try resolver.load()
        let second = try ClientIdentityResolver(backings: [
            ClientIdentityFileStore(directory: dir),
            RefusingBacking(),
        ]).load()
        XCTAssertEqual(second.certPEM, first.certPEM)
    }

    func testSecondaryHitHealsPrimary() throws {
        // An identity that only exists in the secondary (an older Keychain item) is adopted
        // and copied into the durable file, so the next launch does not depend on it.
        let identity = try generateIdentity()
        let existing = dir.appendingPathComponent("secondary", isDirectory: true)
        try ClientIdentityFileStore(directory: existing).saveIdentity(identity)

        let primaryDir = dir.appendingPathComponent("primary", isDirectory: true)
        let resolver = ClientIdentityResolver(backings: [
            ClientIdentityFileStore(directory: primaryDir),
            ClientIdentityFileStore(directory: existing, name: "secondary"),
        ])
        let adopted = try resolver.load()
        XCTAssertEqual(adopted.certPEM, identity.certPEM)

        let healed = try ClientIdentityFileStore(directory: primaryDir).loadIdentity()
        XCTAssertEqual(healed?.certPEM, identity.certPEM)
    }

    // MARK: fail-closed

    func testCorruptStoreThrowsInsteadOfGeneratingAKey() throws {
        let file = dir.appendingPathComponent("client-identity.json")
        try Data("{not json".utf8).write(to: file)

        let resolver = ClientIdentityResolver(backings: [ClientIdentityFileStore(directory: dir)])
        XCTAssertThrowsError(try resolver.load()) { error in
            guard case IdentityStoreError.corrupt = error else {
                return XCTFail("expected .corrupt, got \(error)")
            }
        }
        // The corrupt bytes must survive: no silent replacement, no phantom pairing.
        XCTAssertEqual(try Data(contentsOf: file), Data("{not json".utf8))
    }

    func testEmptyFieldsAreCorrupt() throws {
        let file = dir.appendingPathComponent("client-identity.json")
        try Data(#"{"certPEM":"","keyPEM":""}"#.utf8).write(to: file)
        XCTAssertThrowsError(
            try ClientIdentityResolver(backings: [ClientIdentityFileStore(directory: dir)]).load()
        ) { error in
            guard case IdentityStoreError.corrupt = error else {
                return XCTFail("expected .corrupt, got \(error)")
            }
        }
    }

    func testUnpersistableIdentityFailsInsteadOfReturningMemoryOnlyKey() throws {
        // Read the primary from a directory that cannot be created (a file in the way):
        // the mint must fail closed rather than hand back an identity the next launch
        // will not have.
        let blocker = dir.appendingPathComponent("blocked")
        try Data("x".utf8).write(to: blocker)
        let store = ClientIdentityFileStore(directory: blocker)
        XCTAssertThrowsError(try store.saveIdentity(try generateIdentity())) { error in
            guard case IdentityStoreError.notPersisted = error else {
                return XCTFail("expected .notPersisted, got \(error)")
            }
        }
        let resolver = ClientIdentityResolver(backings: [store])
        XCTAssertThrowsError(try resolver.load()) { error in
            guard case IdentityStoreError.notPersisted = error else {
                return XCTFail("expected .notPersisted, got \(error)")
            }
        }
    }

    func testMintWritesPrimaryAndSecondary() throws {
        let secondary = RecordingBacking(name: "recording")
        let resolver = ClientIdentityResolver(backings: [
            ClientIdentityFileStore(directory: dir),
            secondary,
        ])
        let identity = try resolver.load()
        XCTAssertEqual(secondary.box.saved.count, 1)
        XCTAssertEqual(secondary.box.saved.first?.certPEM, identity.certPEM)
    }
}
