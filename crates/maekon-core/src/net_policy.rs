//! Internal / private / loopback address classification (#7723).
//!
//! Before this module existed, three call sites independently hand-rolled the
//! same `fc00::/7` (IPv6 Unique Local Address) bit-math plus overlapping
//! private-range checks: the SSRF blocklist in `src-tauri/src/feature_capabilities.rs`,
//! the remote-discovery guard in `maekon-web`'s `ai_model_catalog_endpoint.rs`, and
//! the local/private network-target guard next to this file in
//! `config/sections/privacy.rs`. A fourth site (`maekon-audio`'s `cloud_stt.rs`)
//! hand-parsed a loopback host string because that crate intentionally carries no
//! URL-parsing dependency. Over time the `maekon-web` copy had already drifted —
//! it additionally rejects CGNAT (100.64.0.0/10), the NAT64 well-known prefix, and
//! multicast, while the other two do not.
//!
//! This module is the single source of truth for the shared bit-math primitives.
//! It does **not** collapse the three sites onto one policy — their differences
//! are real security decisions, not accidents — so each site keeps its own exact
//! accept/reject set, expressed either as a direct primitive call or as an
//! [`InternalRangePolicy`] with explicit `with_*` opt-ins for the extra ranges it
//! alone rejects.
//!
//! std-only: no new dependency. Everything here operates on
//! `std::net::{IpAddr, Ipv4Addr, Ipv6Addr}` or plain `&str` host/authority text.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

// ── Shared bit-math primitives ──────────────────────────────────────

/// `fc00::/7` — IPv6 Unique Local Address range (RFC 4193).
///
/// Checked via the raw segment bits rather than the nightly-only
/// `Ipv6Addr::is_unique_local()`, matching this workspace's stable-toolchain /
/// MSRV policy (see the sibling `is_link_local_ipv6` for the same rationale).
pub fn is_unique_local_ipv6(v6: &Ipv6Addr) -> bool {
    (v6.segments()[0] & 0xfe00) == 0xfc00
}

/// `fe80::/10` — IPv6 link-local unicast range.
///
/// Checked via raw octet bits rather than the nightly-only
/// `Ipv6Addr::is_unicast_link_local()`.
pub fn is_link_local_ipv6(v6: &Ipv6Addr) -> bool {
    let octets = v6.octets();
    octets[0] == 0xfe && (octets[1] & 0xc0) == 0x80
}

/// `100.64.0.0/10` — IPv4 Carrier-Grade NAT (CGNAT) range (RFC 6598).
pub fn is_cgnat_ipv4(v4: &Ipv4Addr) -> bool {
    let octets = v4.octets();
    octets[0] == 100 && (octets[1] & 0xc0) == 0x40
}

/// `64:ff9b::/96` — IANA well-known NAT64 prefix (RFC 6146).
///
/// Distinct from [`Ipv6Addr::to_ipv4_mapped`]'s `::ffff:0:0/96` prefix: this
/// prefix reaches internal IPv4 addresses through a NAT64 translator and is NOT
/// caught by `to_ipv4_mapped()`, so it must be checked explicitly.
pub fn is_nat64_well_known_ipv6(v6: &Ipv6Addr) -> bool {
    v6.octets()[..4] == [0x00, 0x64, 0xff, 0x9b]
}

/// Any address in `0.0.0.0/8` ("this network", RFC 1122 §3.2.1.3).
///
/// Broader than the single unspecified address `0.0.0.0` — every `0.x.x.x`
/// address is non-routable in this range. One call site
/// (`config/sections/privacy.rs`'s local-network-host guard) treats the whole
/// `/8` as local; kept as its own named primitive so that policy stays visible.
pub fn is_ipv4_this_network(v4: &Ipv4Addr) -> bool {
    v4.octets()[0] == 0
}

// ── Loopback (IP-level) ──────────────────────────────────────────────

/// `IpAddr::is_loopback`, but an IPv4-mapped IPv6 loopback (`::ffff:127.0.0.1`)
/// is ALSO treated as loopback.
///
/// `std`'s `is_loopback()` does not do this for V6 by design. Use this variant
/// only where that broader mapped-address recognition is the intended policy
/// (e.g. a dual-stack bind trust check); callers that must fail-closed against
/// mapped-address smuggling should keep using plain `IpAddr::is_loopback()` /
/// the stricter URL-level `maekon_network::http_client::host_is_loopback`.
pub fn is_loopback_ip_with_mapped(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_loopback(),
        IpAddr::V6(v6) => {
            v6.is_loopback() || v6.to_ipv4_mapped().is_some_and(|v4| v4.is_loopback())
        }
    }
}

// ── Loopback (pure host-string level, no URL-parser dependency) ─────

/// Extract the host portion from a `host[:port]` or bracketed-IPv6
/// `[host]:port` authority string, without a URL-parser dependency.
///
/// Strips a trailing `:port` (or, for a bracketed IPv6 literal, the brackets
/// themselves). Does NOT strip a scheme or userinfo — callers that have those
/// (e.g. a full `scheme://user:pass@host:port/path` endpoint) must strip them
/// first; see `maekon-audio`'s `cloud_stt::endpoint_is_secure` for that
/// composition.
pub fn authority_host(authority: &str) -> &str {
    if let Some(stripped) = authority.strip_prefix('[') {
        stripped.split(']').next().unwrap_or("")
    } else {
        authority.split(':').next().unwrap_or(authority)
    }
}

/// True when `host` (already extracted — no scheme, userinfo, port, or
/// brackets) is the literal `localhost` or parses as a loopback IP address
/// (`127.0.0.0/8`, `::1`).
///
/// Pure string/std parsing — no URL-parser dependency, for crates that
/// intentionally don't carry one (e.g. `maekon-audio`).
pub fn is_loopback_host_str(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost")
        || host.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

// ── InternalRangePolicy ───────────────────────────────────────────────

/// Named, explicit policy for "is this address internal / must never be
/// treated as a reachable external egress target" classification.
///
/// Two call sites in this workspace apply the same baseline blocklist
/// (loopback + RFC 1918 private + IPv4 link-local + unspecified + IPv6
/// unique-local, with IPv4-mapped IPv6 unwrapped and re-checked against the
/// IPv4 rules) but diverge on extra ranges. Rather than silently picking one
/// behavior, the extra ranges are opt-in flags here so a reader can see
/// exactly which policy a given call site enforces — and so future drift
/// requires an explicit flag flip, not a re-implementation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InternalRangePolicy {
    pub include_cgnat: bool,
    pub include_nat64: bool,
    pub include_multicast: bool,
    pub include_broadcast: bool,
    pub include_documentation: bool,
    pub include_ipv6_link_local: bool,
}

impl InternalRangePolicy {
    /// The shared baseline with every extra range disabled: IPv4
    /// loopback/private/link-local/unspecified, IPv6 loopback/unspecified/
    /// unique-local(`fc00::/7`), and IPv4-mapped IPv6 unwrapped + re-checked
    /// against the IPv4 rules.
    pub const fn minimal() -> Self {
        Self {
            include_cgnat: false,
            include_nat64: false,
            include_multicast: false,
            include_broadcast: false,
            include_documentation: false,
            include_ipv6_link_local: false,
        }
    }

    /// Matches `src-tauri/src/feature_capabilities.rs`'s `is_blocked_probe_address`
    /// (#7698 S2 SSRF blocklist for the provider-endpoint probe): the minimal
    /// baseline, no extra ranges.
    pub const fn ssrf_blocklist() -> Self {
        Self::minimal()
    }

    /// Matches `maekon-web`'s `ai_model_catalog_endpoint.rs`'s `is_internal_ip`
    /// (#6894 SSRF guard for the integration model-discovery path): the baseline
    /// plus CGNAT, the NAT64 well-known prefix, IPv6 multicast, IPv4 broadcast,
    /// IPv4 documentation ranges, and IPv6 link-local (`fe80::/10`) — the
    /// stricter set this call site has always enforced.
    pub const fn strict_remote_discovery_guard() -> Self {
        Self {
            include_cgnat: true,
            include_nat64: true,
            include_multicast: true,
            include_broadcast: true,
            include_documentation: true,
            include_ipv6_link_local: true,
        }
    }

    pub const fn with_cgnat(mut self) -> Self {
        self.include_cgnat = true;
        self
    }

    pub const fn with_nat64(mut self) -> Self {
        self.include_nat64 = true;
        self
    }

    pub const fn with_multicast(mut self) -> Self {
        self.include_multicast = true;
        self
    }

    pub const fn with_broadcast(mut self) -> Self {
        self.include_broadcast = true;
        self
    }

    pub const fn with_documentation(mut self) -> Self {
        self.include_documentation = true;
        self
    }

    pub const fn with_ipv6_link_local(mut self) -> Self {
        self.include_ipv6_link_local = true;
        self
    }

    /// Classify `ip` against this policy.
    pub fn is_internal(&self, ip: IpAddr) -> bool {
        match ip {
            IpAddr::V4(v4) => self.is_internal_v4(v4),
            IpAddr::V6(v6) => {
                v6.is_loopback()
                    || v6.is_unspecified()
                    || is_unique_local_ipv6(&v6)
                    || v6
                        .to_ipv4_mapped()
                        .is_some_and(|v4| self.is_internal_v4(v4))
                    || (self.include_ipv6_link_local && is_link_local_ipv6(&v6))
                    || (self.include_multicast && v6.is_multicast())
                    || (self.include_nat64 && is_nat64_well_known_ipv6(&v6))
            }
        }
    }

    fn is_internal_v4(&self, v4: Ipv4Addr) -> bool {
        v4.is_loopback()
            || v4.is_private()
            || v4.is_link_local()
            || v4.is_unspecified()
            || (self.include_broadcast && v4.is_broadcast())
            || (self.include_documentation && v4.is_documentation())
            || (self.include_cgnat && is_cgnat_ipv4(&v4))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── is_unique_local_ipv6 / is_link_local_ipv6 / is_cgnat_ipv4 / is_nat64_well_known_ipv6 ──

    #[test]
    fn unique_local_ipv6_matches_fc00_slash_7() {
        assert!(is_unique_local_ipv6(&Ipv6Addr::new(
            0xfd00, 0, 0, 0, 0, 0, 0, 1
        )));
        assert!(is_unique_local_ipv6(&Ipv6Addr::new(
            0xfc00, 0, 0, 0, 0, 0, 0, 1
        )));
        assert!(!is_unique_local_ipv6(&Ipv6Addr::LOCALHOST));
        assert!(!is_unique_local_ipv6(&Ipv6Addr::new(
            0xfe80, 0, 0, 0, 0, 0, 0, 1
        )));
    }

    #[test]
    fn link_local_ipv6_matches_fe80_slash_10() {
        assert!(is_link_local_ipv6(&Ipv6Addr::new(
            0xfe80, 0, 0, 0, 0, 0, 0, 1
        )));
        assert!(is_link_local_ipv6(&Ipv6Addr::new(
            0xfebf, 0, 0, 0, 0, 0, 0, 1
        )));
        assert!(!is_link_local_ipv6(&Ipv6Addr::new(
            0xfc00, 0, 0, 0, 0, 0, 0, 1
        )));
        assert!(!is_link_local_ipv6(&Ipv6Addr::LOCALHOST));
    }

    #[test]
    fn cgnat_ipv4_matches_100_64_slash_10() {
        assert!(is_cgnat_ipv4(&Ipv4Addr::new(100, 64, 0, 1)));
        assert!(is_cgnat_ipv4(&Ipv4Addr::new(100, 127, 255, 255)));
        assert!(!is_cgnat_ipv4(&Ipv4Addr::new(100, 63, 0, 1)));
        assert!(!is_cgnat_ipv4(&Ipv4Addr::new(100, 128, 0, 1)));
        assert!(!is_cgnat_ipv4(&Ipv4Addr::new(10, 0, 0, 1)));
    }

    #[test]
    fn nat64_well_known_matches_prefix() {
        // 64:ff9b::a00:1 -> encodes 10.0.0.1 through the NAT64 well-known prefix.
        assert!(is_nat64_well_known_ipv6(&"64:ff9b::a00:1".parse().unwrap()));
        assert!(!is_nat64_well_known_ipv6(&Ipv6Addr::LOCALHOST));
    }

    #[test]
    fn ipv4_this_network_matches_0_slash_8() {
        assert!(is_ipv4_this_network(&Ipv4Addr::new(0, 0, 0, 0)));
        assert!(is_ipv4_this_network(&Ipv4Addr::new(0, 1, 2, 3)));
        assert!(!is_ipv4_this_network(&Ipv4Addr::new(1, 0, 0, 0)));
    }

    // ── is_loopback_ip_with_mapped ──────────────────────────────────

    #[test]
    fn loopback_ip_with_mapped_accepts_v4_and_v6_loopback() {
        assert!(is_loopback_ip_with_mapped(IpAddr::V4(Ipv4Addr::new(
            127, 0, 0, 1
        ))));
        assert!(is_loopback_ip_with_mapped(IpAddr::V6(Ipv6Addr::LOCALHOST)));
    }

    #[test]
    fn loopback_ip_with_mapped_accepts_ipv4_mapped_loopback() {
        let mapped = Ipv4Addr::new(127, 0, 0, 1).to_ipv6_mapped();
        assert!(is_loopback_ip_with_mapped(IpAddr::V6(mapped)));
    }

    #[test]
    fn loopback_ip_with_mapped_rejects_non_loopback() {
        assert!(!is_loopback_ip_with_mapped(IpAddr::V4(Ipv4Addr::new(
            10, 0, 0, 1
        ))));
        let mapped = Ipv4Addr::new(8, 8, 8, 8).to_ipv6_mapped();
        assert!(!is_loopback_ip_with_mapped(IpAddr::V6(mapped)));
    }

    // ── authority_host / is_loopback_host_str ────────────────────────

    #[test]
    fn authority_host_strips_port() {
        assert_eq!(authority_host("localhost:8080"), "localhost");
        assert_eq!(authority_host("127.0.0.1:9000"), "127.0.0.1");
        assert_eq!(authority_host("example.com"), "example.com");
    }

    #[test]
    fn authority_host_strips_ipv6_brackets_and_port() {
        assert_eq!(authority_host("[::1]:9000"), "::1");
        assert_eq!(authority_host("[::1]"), "::1");
    }

    #[test]
    fn loopback_host_str_accepts_localhost_and_loopback_ips() {
        assert!(is_loopback_host_str("localhost"));
        assert!(is_loopback_host_str("LOCALHOST"));
        assert!(is_loopback_host_str("127.0.0.1"));
        assert!(is_loopback_host_str("::1"));
    }

    #[test]
    fn loopback_host_str_rejects_lookalikes_and_remote_hosts() {
        assert!(!is_loopback_host_str("127.0.0.1.evil.com"));
        assert!(!is_loopback_host_str("example.com"));
        assert!(!is_loopback_host_str("10.0.0.5"));
    }

    // ── InternalRangePolicy::ssrf_blocklist — pins feature_capabilities.rs's
    //    is_blocked_probe_address (#7698 S2) BEFORE migration ────────────────

    #[test]
    fn ssrf_blocklist_rejects_ipv4_loopback_private_link_local_unspecified() {
        let policy = InternalRangePolicy::ssrf_blocklist();
        assert!(policy.is_internal(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1))));
        assert!(policy.is_internal(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 5))));
        assert!(policy.is_internal(IpAddr::V4(Ipv4Addr::new(172, 16, 0, 1))));
        assert!(policy.is_internal(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1))));
        assert!(policy.is_internal(IpAddr::V4(Ipv4Addr::new(169, 254, 169, 254))));
        assert!(policy.is_internal(IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0))));
    }

    #[test]
    fn ssrf_blocklist_rejects_ipv6_loopback_unique_local_and_mapped() {
        let policy = InternalRangePolicy::ssrf_blocklist();
        assert!(policy.is_internal(IpAddr::V6(Ipv6Addr::LOCALHOST)));
        assert!(policy.is_internal(IpAddr::V6(Ipv6Addr::new(0xfd00, 0, 0, 0, 0, 0, 0, 1))));
        let mapped = Ipv4Addr::new(169, 254, 169, 254).to_ipv6_mapped();
        assert!(policy.is_internal(IpAddr::V6(mapped)));
    }

    #[test]
    fn ssrf_blocklist_allows_public_addresses() {
        let policy = InternalRangePolicy::ssrf_blocklist();
        assert!(!policy.is_internal(IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8))));
        assert!(!policy.is_internal(IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1))));
    }

    /// Documents the intentional divergence: the minimal/SSRF-blocklist policy
    /// does NOT reject CGNAT / NAT64 / multicast / broadcast / documentation /
    /// IPv6 link-local — only `strict_remote_discovery_guard` does.
    #[test]
    fn ssrf_blocklist_does_not_reject_extra_ranges() {
        let policy = InternalRangePolicy::ssrf_blocklist();
        assert!(!policy.is_internal(IpAddr::V4(Ipv4Addr::new(100, 64, 0, 1)))); // CGNAT
        assert!(!policy.is_internal(IpAddr::V6(Ipv6Addr::new(0xff00, 0, 0, 0, 0, 0, 0, 1)))); // multicast
        assert!(!policy.is_internal(IpAddr::V6(Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 1))));
        // link-local v6
    }

    // ── InternalRangePolicy::strict_remote_discovery_guard — pins
    //    ai_model_catalog_endpoint.rs's is_internal_ip (#6894) BEFORE migration ──

    #[test]
    fn strict_discovery_guard_rejects_internal_ipv4() {
        let policy = InternalRangePolicy::strict_remote_discovery_guard();
        for ip in [
            "127.0.0.1",
            "10.0.0.5",
            "192.168.1.1",
            "172.16.0.1",
            "169.254.169.254",
            "0.0.0.0",
            "100.64.0.1",
        ] {
            assert!(
                policy.is_internal(ip.parse().unwrap()),
                "{ip} must be internal"
            );
        }
    }

    #[test]
    fn strict_discovery_guard_rejects_internal_ipv6() {
        let policy = InternalRangePolicy::strict_remote_discovery_guard();
        for ip in [
            "::1",
            "::",
            "fc00::1",
            "fd12::1",
            "fe80::1",
            "::ffff:127.0.0.1",
            "::ffff:10.0.0.1",
            "64:ff9b::a00:1",
            "64:ff9b::a9fe:a9fe",
        ] {
            assert!(
                policy.is_internal(ip.parse().unwrap()),
                "{ip} must be internal"
            );
        }
    }

    #[test]
    fn strict_discovery_guard_allows_public_addresses() {
        let policy = InternalRangePolicy::strict_remote_discovery_guard();
        for ip in ["8.8.8.8", "1.1.1.1", "2606:4700:4700::1111"] {
            assert!(
                !policy.is_internal(ip.parse().unwrap()),
                "{ip} must be external"
            );
        }
    }

    /// Documents that `strict_remote_discovery_guard` is a strict superset of
    /// `ssrf_blocklist` for every case the baseline already rejects — the
    /// stricter policy never re-opens something the minimal one closes.
    #[test]
    fn strict_discovery_guard_is_superset_of_ssrf_blocklist() {
        let minimal = InternalRangePolicy::ssrf_blocklist();
        let strict = InternalRangePolicy::strict_remote_discovery_guard();
        for ip in [
            IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)),
            IpAddr::V4(Ipv4Addr::new(10, 0, 0, 5)),
            IpAddr::V4(Ipv4Addr::new(169, 254, 169, 254)),
            IpAddr::V6(Ipv6Addr::LOCALHOST),
            IpAddr::V6(Ipv6Addr::new(0xfd00, 0, 0, 0, 0, 0, 0, 1)),
        ] {
            if minimal.is_internal(ip) {
                assert!(
                    strict.is_internal(ip),
                    "{ip} regression: strict must still reject what minimal rejects"
                );
            }
        }
    }
}

/// #10106: operator guards for the two address predicates that decide the egress
/// boundary.
///
/// The first real mutation measurement (#10003, run 30965619494) left `&&` → `||`
/// alive in `is_link_local_ipv6` and `||` → `&&` alive in `is_internal_v4`.
/// Both flips widen or narrow "is this address internal?", which is the check
/// that keeps user data off the public network — the existing tests all used
/// addresses where BOTH sides of the operator agreed, so neither flip changed an
/// outcome.
#[cfg(test)]
mod operator_guard_tests {
    use super::*;

    /// `is_link_local_ipv6` is `octets[0] == 0xfe && (octets[1] & 0xc0) == 0x80`.
    /// Killing `&&` → `||` needs addresses where exactly ONE side holds.
    #[test]
    fn link_local_ipv6_requires_both_octet_conditions() {
        // Both hold → link-local.
        assert!(
            is_link_local_ipv6(&Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 1)),
            "fe80::1 is link-local"
        );

        // First holds, second does NOT (0xfc → 0xfc & 0xc0 == 0xc0, not 0x80).
        // Under `||` this would wrongly report link-local.
        assert!(
            !is_link_local_ipv6(&Ipv6Addr::new(0xfc00, 0, 0, 0, 0, 0, 0, 1)),
            "fc00::1 is unique-local, NOT link-local — first octet alone is not enough"
        );

        // Second holds, first does NOT (0x28 != 0xfe, 0x80 & 0xc0 == 0x80).
        // Under `||` this would wrongly report link-local.
        assert!(
            !is_link_local_ipv6(&Ipv6Addr::new(0x2880, 0, 0, 0, 0, 0, 0, 1)),
            "2880::1 is a public address — second octet alone is not enough"
        );

        // Neither holds.
        assert!(
            !is_link_local_ipv6(&Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 1)),
            "2001:db8::1 is not link-local"
        );
    }

    /// `is_internal_v4` is a disjunction. Killing `||` → `&&` needs an address
    /// that satisfies exactly ONE arm: under `&&` it would be reported EXTERNAL,
    /// which is the direction that leaks.
    #[test]
    fn internal_v4_arms_are_independently_sufficient() {
        let policy = InternalRangePolicy::minimal();

        // Loopback only — not private, not link-local, not unspecified.
        assert!(
            policy.is_internal_v4(Ipv4Addr::new(127, 0, 0, 1)),
            "127.0.0.1 is internal on the loopback arm ALONE"
        );
        // Private only — not loopback.
        assert!(
            policy.is_internal_v4(Ipv4Addr::new(10, 0, 0, 1)),
            "10.0.0.1 is internal on the private arm ALONE"
        );
        // Link-local only.
        assert!(
            policy.is_internal_v4(Ipv4Addr::new(169, 254, 0, 1)),
            "169.254.0.1 is internal on the link-local arm ALONE"
        );
        // Unspecified only.
        assert!(
            policy.is_internal_v4(Ipv4Addr::UNSPECIFIED),
            "0.0.0.0 is internal on the unspecified arm ALONE"
        );

        // Negative control: a public address satisfies no arm.
        assert!(
            !policy.is_internal_v4(Ipv4Addr::new(93, 184, 216, 34)),
            "a public address must not be internal"
        );
    }

    /// The OPTIONAL arms must also be independently sufficient. Both shipped
    /// presets set `include_broadcast` and `include_documentation` to the same
    /// value, so a `||` → `&&` flip between them is invisible under either — the
    /// two must be split with the builders to observe it. Under `&&` a
    /// broadcast-only policy would report 255.255.255.255 as EXTERNAL, which is
    /// the leaking direction.
    #[test]
    fn optional_internal_v4_arms_do_not_depend_on_each_other() {
        let broadcast_only = InternalRangePolicy::minimal().with_broadcast();
        assert!(
            broadcast_only.is_internal_v4(Ipv4Addr::BROADCAST),
            "255.255.255.255 is internal on the broadcast arm ALONE, with documentation off"
        );

        let documentation_only = InternalRangePolicy::minimal().with_documentation();
        assert!(
            documentation_only.is_internal_v4(Ipv4Addr::new(192, 0, 2, 1)),
            "192.0.2.1 is internal on the documentation arm ALONE, with broadcast off"
        );

        // Cross-checks: each arm stays OFF when its own flag is off.
        assert!(
            !documentation_only.is_internal_v4(Ipv4Addr::BROADCAST),
            "broadcast must not be internal when only documentation is enabled"
        );
        assert!(
            !broadcast_only.is_internal_v4(Ipv4Addr::new(192, 0, 2, 1)),
            "documentation must not be internal when only broadcast is enabled"
        );
    }
}
