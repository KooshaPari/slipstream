// Pure parsing of a host's mDNS advert (`_slipstream._udp` TXT) into the values the picker needs.
// Kept separate from `HostDiscovery` so the wire interpretation — pair policy, MAC list, OS chain,
// id fallback — is unit-testable without a live Bonjour browser.
//
// Policy carried by this type matches the adverts in `crates/slipstream-host` discovery: the host
// is the authority. Only an EXPLICIT `pair=optional` permits the reduced-security TOFU path; a
// missing or unknown `pair` value must be treated as "pairing required, not optional" (fail
// closed) because those adverts are unauthenticated and advisory.

import Foundation
import SlipstreamShared

/// The host's stated pairing policy, decoded from the `pair` TXT field.
public enum HostPairingPolicy: String, Sendable, Equatable {
    case required
    case optional
    /// Missing or unrecognized — treated as "pairing required, TOFU not offered".
    case unspecified
}

/// An mDNS advert decoded into connect-time facts. `fingerprintHex` and `osChain` are advisory:
/// mDNS is unauthenticated, so pinning/TOFU still verify the host on connect.
public struct HostAdvert: Sendable, Equatable {
    /// Stable host id (`id` TXT), falling back to the Bonjour instance name.
    public let id: String
    /// Host cert SHA-256 as lowercase hex, or nil when absent.
    public let fingerprintHex: String?
    public let pairingPolicy: HostPairingPolicy
    /// Wake-on-LAN MACs from the comma-separated `mac` TXT field (whitespace trimmed, empties dropped).
    public let macAddresses: [String]
    /// Sanitized OS-identity chain (`linux/fedora/bazzite`), empty when not advertised.
    public let osChain: String

    public init(instanceName: String, txt: [String: String]) {
        let rawID = txt["id"]?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        self.id = rawID.isEmpty ? instanceName : rawID

        let rawFP = txt["fp"]?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        self.fingerprintHex = rawFP.isEmpty ? nil : rawFP.lowercased()

        let pairValue = txt["pair"]?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        self.pairingPolicy = HostPairingPolicy(rawValue: pairValue) ?? .unspecified
        self.macAddresses = Self.macList(txt["mac"])
        self.osChain = sanitizeOsChain(txt["os"] ?? "")
    }

    /// `pair=required` — a client must pair before it can stream.
    public var requiresPairing: Bool { pairingPolicy == .required }
    /// `pair=optional` — only this explicit advert may offer the TOFU "Trust" path.
    public var allowsTofu: Bool { pairingPolicy == .optional }

    /// Split the comma-separated `mac` TXT value into individual addresses.
    public static func macList(_ raw: String?) -> [String] {
        (raw ?? "")
            .split(separator: ",")
            .map { $0.trimmingCharacters(in: .whitespaces) }
            .filter { !$0.isEmpty }
    }
}
