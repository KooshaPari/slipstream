// Pure tests for the mDNS advert decoder: how a host's `_slipstream._udp` TXT record becomes the
// picker's pairing policy, MAC list, OS chain and id. No Bonjour, no network — these pin the
// fail-closed rules that decide whether the UI may offer the reduced-security TOFU path.

import XCTest
@testable import SlipstreamKit

final class HostAdvertTests: XCTestCase {
    func testPairRequiredMarksHostAsRequiringPairingOnly() {
        let advert = HostAdvert(instanceName: "desk", txt: ["pair": "required"])
        XCTAssertEqual(advert.pairingPolicy, .required)
        XCTAssertTrue(advert.requiresPairing)
        XCTAssertFalse(advert.allowsTofu, "pair=required must never offer the TOFU path")
    }

    func testPairOptionalIsTheOnlyTofuAdvert() {
        let advert = HostAdvert(instanceName: "desk", txt: ["pair": "optional"])
        XCTAssertEqual(advert.pairingPolicy, .optional)
        XCTAssertTrue(advert.allowsTofu)
        XCTAssertFalse(advert.requiresPairing)
    }

    func testMissingPairFieldFailsClosed() {
        // Older hosts omit `pair` entirely. Unauthenticated mDNS must not be read as consent to
        // trust on first use — the client treats it as pairing-required (not TOFU).
        let advert = HostAdvert(instanceName: "desk", txt: [:])
        XCTAssertEqual(advert.pairingPolicy, .unspecified)
        XCTAssertFalse(advert.requiresPairing)
        XCTAssertFalse(advert.allowsTofu)
    }

    func testUnknownPairValueFailsClosed() {
        // A future host value we don't know ("pair=maybe") must not silently become TOFU.
        let advert = HostAdvert(instanceName: "desk", txt: ["pair": "maybe"])
        XCTAssertEqual(advert.pairingPolicy, .unspecified)
        XCTAssertFalse(advert.allowsTofu)
        XCTAssertFalse(advert.requiresPairing)
    }

    func testIDFallsBackToInstanceNameWhenAbsentOrBlank() {
        XCTAssertEqual(HostAdvert(instanceName: "Kube Box", txt: [:]).id, "Kube Box")
        XCTAssertEqual(HostAdvert(instanceName: "Kube Box", txt: ["id": "  "]).id, "Kube Box")
        XCTAssertEqual(HostAdvert(instanceName: "Kube Box", txt: ["id": "abc"]).id, "abc")
    }

    func testPairValueIsTrimmedBeforeMatching() {
        let advert = HostAdvert(instanceName: "desk", txt: ["pair": " required "])
        XCTAssertEqual(advert.pairingPolicy, .required)
        XCTAssertFalse(advert.allowsTofu, "pair=required must never offer the TOFU path")
    }

    func testFingerprintIsTrimmedLowercasedAndNilWhenBlank() {
        let hex = String(repeating: "AB", count: 32)
        XCTAssertEqual(
            HostAdvert(instanceName: "h", txt: ["fp": " \(hex) "]).fingerprintHex,
            hex.lowercased())
        XCTAssertNil(HostAdvert(instanceName: "h", txt: ["fp": ""]).fingerprintHex)
        XCTAssertNil(HostAdvert(instanceName: "h", txt: [:]).fingerprintHex)
    }

    func testMACListSplitsTrimsAndDropsEmpties() {
        XCTAssertEqual(
            HostAdvert(instanceName: "h", txt: ["mac": " aa:bb:cc:dd:ee:ff , 11:22:33:44:55:66 "])
                .macAddresses,
            ["aa:bb:cc:dd:ee:ff", "11:22:33:44:55:66"])
        XCTAssertEqual(
            HostAdvert(instanceName: "h", txt: ["mac": "aa:bb:cc:dd:ee:ff,, "]).macAddresses,
            ["aa:bb:cc:dd:ee:ff"])
        XCTAssertTrue(HostAdvert(instanceName: "h", txt: [:]).macAddresses.isEmpty)
        XCTAssertTrue(HostAdvert.macList(nil).isEmpty)
    }

    func testOSChainIsSanitizedThroughTheSharedRule() {
        let advert = HostAdvert(instanceName: "h", txt: ["os": "Linux/Fedora/Bazzite!"])
        XCTAssertEqual(advert.osChain, "linux/fedora/bazzite")
        XCTAssertEqual(HostAdvert(instanceName: "h", txt: [:]).osChain, "")
    }

    func testRealisticAdvertDecodesEveryField() {
        let advert = HostAdvert(instanceName: "Slipstream on koosha-tower", txt: [
            "proto": "slipstream/1",
            "id": "0f3c-uuid",
            "fp": String(repeating: "9f", count: 32),
            "pair": "required",
            "mac": "aa:bb:cc:dd:ee:ff",
            "os": "linux/arch/cachyos",
        ])
        XCTAssertEqual(advert.id, "0f3c-uuid")
        XCTAssertEqual(advert.fingerprintHex, String(repeating: "9f", count: 32))
        XCTAssertTrue(advert.requiresPairing)
        XCTAssertFalse(advert.allowsTofu)
        XCTAssertEqual(advert.macAddresses, ["aa:bb:cc:dd:ee:ff"])
        XCTAssertEqual(advert.osChain, "linux/arch/cachyos")
    }
}
