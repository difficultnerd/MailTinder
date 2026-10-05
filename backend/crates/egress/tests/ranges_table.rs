//! ASVS V1.3.6: the refused-address table and the property tests that every
//! address in a refused IPv4 range is refused, and that IPv4-mapped IPv6
//! matches the IPv4 verdict.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::enum_glob_use,
    clippy::too_many_lines,
    clippy::doc_markdown
)]

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use egress::classify_ip;
use ports::RefusedRange;
use proptest::prelude::*;

fn ipv4(s: &str) -> IpAddr {
    IpAddr::V4(s.parse::<Ipv4Addr>().expect("valid ipv4"))
}

fn ipv6(s: &str) -> IpAddr {
    IpAddr::V6(s.parse::<Ipv6Addr>().expect("valid ipv6"))
}

/// One row per refused range in the algorithm, with a sample address and the
/// exact variant the user sees. Google's IPv6 metadata address (`fd20:ce::254`)
/// is reported as UniqueLocal.
#[test]
fn asvs_v1_3_6_refused_ranges_table() {
    let refused: &[(IpAddr, RefusedRange)] = &[
        (ipv4("0.0.0.1"), RefusedRange::Unspecified),
        (ipv4("127.0.0.1"), RefusedRange::Loopback),
        (ipv4("10.0.0.1"), RefusedRange::Private),
        (ipv4("172.16.0.1"), RefusedRange::Private),
        (ipv4("192.168.1.1"), RefusedRange::Private),
        (ipv4("100.64.0.1"), RefusedRange::Cgnat),
        (ipv4("169.254.169.254"), RefusedRange::Metadata),
        (ipv4("169.254.1.1"), RefusedRange::LinkLocal),
        (ipv4("192.0.0.1"), RefusedRange::Reserved),
        (ipv4("192.0.2.1"), RefusedRange::Documentation),
        (ipv4("198.51.100.1"), RefusedRange::Documentation),
        (ipv4("203.0.113.1"), RefusedRange::Documentation),
        (ipv4("198.18.0.1"), RefusedRange::Benchmarking),
        (ipv4("192.88.99.1"), RefusedRange::Translation),
        (ipv4("224.0.0.1"), RefusedRange::Multicast),
        (ipv4("255.255.255.255"), RefusedRange::Broadcast),
        (ipv4("240.0.0.1"), RefusedRange::Reserved),
        (ipv6("::"), RefusedRange::Unspecified),
        (ipv6("::1"), RefusedRange::Loopback),
        (ipv6("::ffff:127.0.0.1"), RefusedRange::Loopback),
        (ipv6("::ffff:169.254.169.254"), RefusedRange::Metadata),
        (ipv6("::ffff:10.0.0.1"), RefusedRange::Private),
        (ipv6("::127.0.0.1"), RefusedRange::Translation), // IPv4-compatible ::/96
        (ipv6("64:ff9b::7f00:1"), RefusedRange::Translation), // NAT64
        (ipv6("64:ff9b:1::1"), RefusedRange::Translation), // NAT64
        (ipv6("2002:7f00:1::"), RefusedRange::Translation), // 6to4
        (ipv6("2001:0:4136:e378::1"), RefusedRange::Translation), // Teredo
        (ipv6("2001:db8::1"), RefusedRange::Documentation),
        (ipv6("2001:1::1"), RefusedRange::Reserved), // 2001::/23 IETF
        (ipv6("fd20:ce::254"), RefusedRange::UniqueLocal), // fc00::/7
        (ipv6("fe80::1"), RefusedRange::LinkLocal),
        (ipv6("fec0::1"), RefusedRange::Private), // site-local
        (ipv6("ff00::1"), RefusedRange::Multicast),
        (ipv6("100::1"), RefusedRange::Reserved),
        (ipv6("4000::1"), RefusedRange::Reserved), // outside 2000::/3
    ];
    for (ip, expected) in refused {
        assert_eq!(classify_ip(*ip), Err(*expected), "for {ip}");
    }
    // Globally routable samples (never contacted, only classified).
    assert_eq!(classify_ip(ipv4("93.184.216.34")), Ok(()));
    assert_eq!(classify_ip(ipv6("2606:4700::1")), Ok(()));
}

/// The refused IPv4 ranges, kept in sync with `ranges.rs`.
fn refused_v4_ranges() -> Vec<(u32, u8)> {
    vec![
        (0x0000_0000, 8),
        (0x7f00_0000, 8),
        (0x0a00_0000, 8),
        (0xac10_0000, 12),
        (0xc0a8_0000, 16),
        (0x6440_0000, 10),
        (0xa9fe_a9fe, 32),
        (0xa9fe_0000, 16),
        (0xc000_0000, 24),
        (0xc000_0200, 24),
        (0xc633_6400, 24),
        (0xcb00_7100, 24),
        (0xc612_0000, 15),
        (0xc058_6300, 24),
        (0xe000_0000, 4),
        (0xffff_ffff, 32),
        (0xf000_0000, 4),
    ]
}

/// A strategy that generates a random host in one of the refused IPv4 ranges.
fn refused_v4_address() -> impl Strategy<Value = Ipv4Addr> {
    let ranges = refused_v4_ranges();
    (0..ranges.len()).prop_flat_map(move |i| {
        let (network, prefix) = ranges[i];
        let host_mask = if prefix == 32 { 0 } else { u32::MAX >> prefix };
        prop::num::u32::ANY
            .prop_map(move |r| Ipv4Addr::from((network & !host_mask) | (r & host_mask)))
    })
}

proptest! {
    /// Every address inside a refused IPv4 range is refused, no matter the host
    /// bits (ASVS V1.3.6).
    #[test]
    fn asvs_v1_3_6_every_address_in_refused_v4_ranges_is_refused(ip in refused_v4_address()) {
        prop_assert!(classify_ip(IpAddr::V4(ip)).is_err(), "{ip} must be refused");
    }

    /// IPv4-mapped IPv6 must reach the same verdict as the embedded IPv4.
    #[test]
    fn asvs_v1_3_6_ipv4_mapped_matches_ipv4_verdict(a in prop::num::u32::ANY) {
        let v4 = Ipv4Addr::from(a);
        let direct = classify_ip(IpAddr::V4(v4));
        let mapped = classify_ip(IpAddr::V6(v4.to_ipv6_mapped()));
        prop_assert_eq!(direct, mapped);
    }
}
