// Tests for the connect-attempt decision legs that SessionModel delegates to the kit: the
// admission table after a successful handshake (stream / confirm TOFU / refuse-unpaired) and the
// classification + wording of every failure a user actually hits (host unreachable, pair
// rejected, no approval, version mismatch). Pure value logic — no host, no network.

import SlipstreamCore
import XCTest
@testable import SlipstreamKit

final class ConnectOutcomeTests: XCTestCase {
    // MARK: - Admission after a successful handshake

    func testPinnedAutoTrustOrApprovedRequestAlwaysStreams() {
        for allowsTofu in [true, false] {
            XCTAssertEqual(
                ConnectionAdmission.decide(
                    pinned: true, autoTrust: false, requestAccess: false, hostAllowsTofu: allowsTofu),
                .stream, "a pinned host never needs a trust prompt")
            XCTAssertEqual(
                ConnectionAdmission.decide(
                    pinned: false, autoTrust: true, requestAccess: false, hostAllowsTofu: allowsTofu),
                .stream)
            XCTAssertEqual(
                ConnectionAdmission.decide(
                    pinned: false, autoTrust: false, requestAccess: true, hostAllowsTofu: allowsTofu),
                .stream, "an operator-approved request-access connect is already trusted")
        }
    }

    func testUnpinnedTofuHostAsksForConfirmation() {
        XCTAssertEqual(
            ConnectionAdmission.decide(
                pinned: false, autoTrust: false, requestAccess: false, hostAllowsTofu: true),
            .confirmTrust)
    }

    func testUnpinnedPairRequiredHostIsRefusedAndDropped() {
        let admission = ConnectionAdmission.decide(
            pinned: false, autoTrust: false, requestAccess: false, hostAllowsTofu: false)
        XCTAssertEqual(admission, .refuseUnpaired)
        XCTAssertTrue(admission.requiresDrop, "an unpaired, non-TOFU connection must be closed")
        XCTAssertFalse(ConnectionAdmission.stream.requiresDrop)
        XCTAssertFalse(ConnectionAdmission.confirmTrust.requiresDrop)
    }

    // MARK: - Failure classification

    func testEveryTypedRejectionSurvivesClassification() {
        let cases: [(Int32, HostRejection)] = [
            (SLIPSTREAM_STATUS_REJECTED_NOT_ARMED.rawValue, .pairingNotArmed),
            (SLIPSTREAM_STATUS_REJECTED_BOUND_OTHER.rawValue, .pairingBoundToOtherDevice),
            (SLIPSTREAM_STATUS_REJECTED_RATE_LIMITED.rawValue, .pairingRateLimited),
            (SLIPSTREAM_STATUS_REJECTED_IDENTITY_REQUIRED.rawValue, .identityRequired),
            (SLIPSTREAM_STATUS_REJECTED_DENIED.rawValue, .denied),
            (SLIPSTREAM_STATUS_REJECTED_APPROVAL_TIMEOUT.rawValue, .approvalTimeout),
            (SLIPSTREAM_STATUS_REJECTED_SUPERSEDED.rawValue, .superseded),
            (SLIPSTREAM_STATUS_REJECTED_WIRE_VERSION.rawValue, .wireVersionMismatch),
            (SLIPSTREAM_STATUS_REJECTED_BUSY.rawValue, .busy),
        ]
        for (code, expected) in cases {
            XCTAssertEqual(HostRejection(status: code), expected, "status \(code)")
            XCTAssertEqual(
                ConnectionFailure.classify(SlipstreamClientError.status(code)), .dismissed(expected),
                "a raw status must still surface as the stated reason")
        }
    }

    func testUnknownStatusStaysNumericAndIsNotARejection() {
        XCTAssertNil(HostRejection(status: -4242))
        XCTAssertEqual(
            ConnectionFailure.classify(SlipstreamClientError.status(-4242)), .unexpected(-4242))
    }

    func testLocalErrorsMapToTheirOwnOutcomes() {
        XCTAssertEqual(ConnectionFailure.classify(SlipstreamClientError.connectFailed), .unreachable)
        XCTAssertEqual(ConnectionFailure.classify(SlipstreamClientError.invalidPin), .badPin)
        XCTAssertEqual(ConnectionFailure.classify(SlipstreamClientError.wrongPIN), .wrongPIN)
        XCTAssertEqual(ConnectionFailure.classify(SlipstreamClientError.closed), .closed)
        XCTAssertEqual(
            ConnectionFailure.classify(SlipstreamClientError.rejected(.busy)), .dismissed(.busy))
    }

    func testNonClientErrorIsUnreachableNotARejection() {
        struct Odd: Error {}
        XCTAssertEqual(ConnectionFailure.classify(Odd()), .unreachable)
    }

    // MARK: - Wording

    func testUnreachableHintNamesEndpointOnlyWhenUnpinned() {
        let unpinned = ConnectionFailure.unreachable.message(
            hostName: "tower", pinned: false, endpoint: "10.0.0.4:9777")
        XCTAssertTrue(unpinned.contains("10.0.0.4:9777"), "the hint should show where it dialled")
        XCTAssertTrue(unpinned.lowercased().contains("pair"), "point at pairing when unpaired")

        let pinned = ConnectionFailure.unreachable.message(hostName: "tower", pinned: true)
        XCTAssertTrue(pinned.contains("pinned fingerprint"))
        XCTAssertFalse(pinned.contains("10.0.0.4"), "a pinned host keeps its saved address")
    }

    func testApprovalTimeoutTellsTheUserToApproveOnTheHost() {
        let text = ConnectionFailure.dismissed(.approvalTimeout)
            .message(hostName: "tower", pinned: false)
        XCTAssertTrue(text.hasPrefix("tower: "))
        XCTAssertTrue(text.lowercased().contains("approve"))
    }

    func testVersionMismatchIsNeverReportedAsUnreachable() {
        let text = ConnectionFailure.classify(
            SlipstreamClientError.rejected(.wireVersionMismatch)
        ).message(hostName: "tower", pinned: true)
        XCTAssertFalse(text.lowercased().contains("unreachable"))
        XCTAssertTrue(text.lowercased().contains("version"))
    }

    func testEveryFailureWordingIsDistinctAndNamesTheHost() {
        let failures: [ConnectionFailure] = [
            .unreachable, .badPin, .wrongPIN, .closed, .unexpected(7),
            .dismissed(.pairingNotArmed), .dismissed(.pairingBoundToOtherDevice),
            .dismissed(.pairingRateLimited), .dismissed(.identityRequired), .dismissed(.denied),
            .dismissed(.approvalTimeout), .dismissed(.superseded), .dismissed(.wireVersionMismatch),
            .dismissed(.busy),
        ]
        var seen = Set<String>()
        for failure in failures {
            let text = failure.message(hostName: "tower", pinned: false, endpoint: "h:9777")
            XCTAssertFalse(text.isEmpty)
            XCTAssertTrue(text.contains("tower"), "every alert names the host: \(failure)")
            XCTAssertTrue(seen.insert(text).inserted, "duplicate wording for \(failure)")
        }
    }
}
