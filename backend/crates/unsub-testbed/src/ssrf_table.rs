//! The SSRF host table (S10 6.2), used by T-702 with a fake resolver.
//!
//! Each case is a host name under `.test` (RFC 2606) mapped to the addresses a
//! fake resolver returns for it. No entry is ever dialled: `93.184.216.34` is
//! used only as a "public" answer and is never contacted.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// One host name and the addresses a fake resolver answers with for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SsrfCase {
    /// The host name under `.test` the fake resolver maps.
    pub host: &'static str,
    /// Every address the fake resolver returns for `host`, in order.
    pub resolves_to: &'static [IpAddr],
    /// Why the case exists, in the words of S10 6.2.
    pub note: &'static str,
}

const fn v4(a: u8, b: u8, c: u8, d: u8) -> IpAddr {
    IpAddr::V4(Ipv4Addr::new(a, b, c, d))
}

const fn v6(segments: [u16; 8]) -> IpAddr {
    IpAddr::V6(Ipv6Addr::new(
        segments[0],
        segments[1],
        segments[2],
        segments[3],
        segments[4],
        segments[5],
        segments[6],
        segments[7],
    ))
}

/// The table S10 6.2 asks for, one case per refused range.
///
/// `metadata.google.internal` is not here: T-702 tests it by name.
pub const SSRF_CASES: &[SsrfCase] = &[
    SsrfCase {
        host: "loopback4.test",
        resolves_to: &[v4(127, 0, 0, 1)],
        note: "IPv4 loopback 127.0.0.0/8",
    },
    SsrfCase {
        host: "private10.test",
        resolves_to: &[v4(10, 0, 0, 1)],
        note: "private 10.0.0.0/8",
    },
    SsrfCase {
        host: "private172.test",
        resolves_to: &[v4(172, 16, 0, 1)],
        note: "private 172.16.0.0/12",
    },
    SsrfCase {
        host: "private192.test",
        resolves_to: &[v4(192, 168, 1, 1)],
        note: "private 192.168.0.0/16",
    },
    SsrfCase {
        host: "cgnat.test",
        resolves_to: &[v4(100, 64, 0, 1)],
        note: "CGNAT 100.64.0.0/10",
    },
    SsrfCase {
        host: "linklocal.test",
        resolves_to: &[v4(169, 254, 1, 1)],
        note: "link-local 169.254.0.0/16",
    },
    SsrfCase {
        host: "metadata-ip.test",
        resolves_to: &[v4(169, 254, 169, 254)],
        note: "cloud metadata address inside link-local",
    },
    SsrfCase {
        host: "loopback6.test",
        resolves_to: &[v6([0, 0, 0, 0, 0, 0, 0, 1])],
        note: "IPv6 loopback ::1",
    },
    SsrfCase {
        host: "linklocal6.test",
        resolves_to: &[v6([0xfe80, 0, 0, 0, 0, 0, 0, 1])],
        note: "IPv6 link-local fe80::/10",
    },
    SsrfCase {
        host: "ula6.test",
        resolves_to: &[v6([0xfd00, 0, 0, 0, 0, 0, 0, 1])],
        note: "IPv6 unique local fc00::/7",
    },
    SsrfCase {
        host: "mapped6.test",
        resolves_to: &[v6([0, 0, 0, 0, 0, 0xffff, 0x7f00, 0x0001])],
        note: "IPv4-mapped IPv6 ::ffff:127.0.0.1",
    },
    SsrfCase {
        host: "mixed.test",
        resolves_to: &[v4(93, 184, 216, 34), v4(10, 0, 0, 1)],
        note: "public then private on the second answer; the first is pinned",
    },
];
