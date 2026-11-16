// The two pure decisions a connect attempt makes, extracted from the app's SessionModel so they
// can be tested without a host, a network, or a MainActor view model:
//
//  1. `ConnectionAdmission` — what a COMPLETED handshake means for trust: stream now, show the
//     TOFU confirmation, or refuse because the host requires pairing and this attempt was unpinned.
//  2. `ConnectionFailure` — how a FAILED attempt is classified and worded for the user, so a host
//     that answered with a typed rejection is never reported as "unreachable".
//
// Both are pure; SessionModel calls them and maps the result onto its phase/errorMessage.

import Foundation
import SlipstreamShared

/// What to do with a live connection whose handshake succeeded.
public enum ConnectionAdmission: Equatable, Sendable {
    /// Trust for this attempt is already established (pin present, auto-trust, or an approved
    /// request-access grant) — start streaming immediately.
    case stream
    /// Unpinned host that explicitly advertises `pair=optional` — keep the stream live behind the
    /// TOFU confirmation prompt.
    case confirmTrust
    /// Unpinned host that did NOT permit TOFU — drop the connection; fail closed.
    case refuseUnpaired

    /// Decision table for a successful handshake.
    public static func decide(
        pinned: Bool, autoTrust: Bool, requestAccess: Bool, hostAllowsTofu: Bool
    ) -> ConnectionAdmission {
        if pinned || autoTrust || requestAccess { return .stream }
        return hostAllowsTofu ? .confirmTrust : .refuseUnpaired
    }

    /// True when the caller must close the connection it just opened.
    public var requiresDrop: Bool { self == .refuseUnpaired }
}

/// A failed connect/pair attempt, classified for surfacing.
public enum ConnectionFailure: Equatable, Sendable {
    /// The host deliberately turned the attempt away and said why (typed QUIC application close).
    case dismissed(HostRejection)
    /// The caller supplied a pin that was not 32 bytes — a local fail-closed bug, not a network fault.
    case badPin
    /// The SPAKE2 pairing proof was rejected (wrong PIN during the ceremony).
    case wrongPIN
    /// The connection was closed underneath us (host restarted, session superseded).
    case closed
    /// No usable answer: wrong host/port, timeout, host down, or a certificate-pin mismatch.
    case unreachable
    /// A status code we don't recognize — surfaced numerically, never as a rejection reason.
    case unexpected(Int32)

    /// Classify any error thrown by `SlipstreamConnection.connect` / `pair`.
    public static func classify(_ error: Error) -> ConnectionFailure {
        guard let error = error as? SlipstreamClientError else { return .unreachable }
        switch error {
        case .rejected(let rejection):
            return .dismissed(rejection)
        case .invalidPin:
            return .badPin
        case .wrongPIN:
            return .wrongPIN
        case .closed:
            return .closed
        case .connectFailed:
            return .unreachable
        case .status(let code):
            if let rejection = HostRejection(status: code) { return .dismissed(rejection) }
            return .unexpected(code)
        }
    }

    /// User-facing sentence for the connect alert. `endpoint` is `host:port`, used only in the
    /// unreachable hint (never for a stated rejection — those already name the fix).
    public func message(hostName: String, pinned: Bool, endpoint: String? = nil) -> String {
        switch self {
        case .dismissed(let rejection):
            return "\(hostName): \(rejection.userMessage)"
        case .badPin:
            return "\(hostName): this host's saved fingerprint is corrupt (not a 32-byte SHA-256) "
                + "— unpin it on the host card and pair again."
        case .wrongPIN:
            return "\(hostName) rejected that PIN — check the code shown on the host."
        case .closed:
            return "\(hostName) closed the session — it may have restarted or another device took over."
        case .unexpected(let code):
            return "\(hostName) refused the connection (status \(code))."
        case .unreachable:
            if pinned {
                return "Could not connect to \(hostName) — host unreachable, not running, its "
                    + "identity no longer matches the pinned fingerprint, or it requires pairing "
                    + "and no longer recognizes this Mac (right-click the host card to pair again)."
            }
            let where_ = endpoint.map { " on \($0)" } ?? ""
            return "Could not connect to \(hostName) — is slipstream-host running\(where_)? "
                + "If it requires pairing, right-click the host card and pair with its PIN first."
        }
    }
}
