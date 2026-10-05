//! Refused address ranges for the SSRF checks: `classify_ip`.
//!
//! The table is written once as data and checked in order with prefix masking;
//! no address is parsed from a string at check time.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use ports::RefusedRange;

/// One refused IPv4 range: `(network_bits, prefix_len, variant)`.
const V4: &[(u32, u8, RefusedRange)] = &[
    (0x0000_0000, 8, RefusedRange::Unspecified), // 0.0.0.0/8
    (0x7f00_0000, 8, RefusedRange::Loopback),    // 127.0.0.0/8
    (0x0a00_0000, 8, RefusedRange::Private),     // 10.0.0.0/8
    (0xac10_0000, 12, RefusedRange::Private),    // 172.16.0.0/12
    (0xc0a8_0000, 16, RefusedRange::Private),    // 192.168.0.0/16
    (0x6440_0000, 10, RefusedRange::Cgnat),      // 100.64.0.0/10
    (0xa9fe_a9fe, 32, RefusedRange::Metadata),   // 169.254.169.254/32 (before the /16)
    (0xa9fe_0000, 16, RefusedRange::LinkLocal),  // 169.254.0.0/16
    (0xc000_0000, 24, RefusedRange::Reserved),   // 192.0.0.0/24
    (0xc000_0200, 24, RefusedRange::Documentation), // 192.0.2.0/24
    (0xc633_6400, 24, RefusedRange::Documentation), // 198.51.100.0/24
    (0xcb00_7100, 24, RefusedRange::Documentation), // 203.0.113.0/24
    (0xc612_0000, 15, RefusedRange::Benchmarking), // 198.18.0.0/15
    (0xc058_6300, 24, RefusedRange::Translation), // 192.88.99.0/24 (6to4 relay)
    (0xe000_0000, 4, RefusedRange::Multicast),   // 224.0.0.0/4
    (0xffff_ffff, 32, RefusedRange::Broadcast),  // 255.255.255.255/32
    (0xf000_0000, 4, RefusedRange::Reserved),    // 240.0.0.0/4
];

/// One refused IPv6 range: `(network_bits, prefix_len, variant)`.
///
/// The IPv4-mapped range (`::ffff:0:0/96`) is handled separately before this
/// table (the embedded IPv4 is classified by `classify_v4`), so it does not
/// appear here. `2001:db8::/32` is listed before `2001::/32` and `2001::/23`
/// so documentation addresses keep their Documentation variant.
const V6: &[(u128, u8, RefusedRange)] = &[
    (
        0x0000_0000_0000_0000_0000_0000_0000_0000,
        128,
        RefusedRange::Unspecified,
    ), // ::/128
    (
        0x0000_0000_0000_0000_0000_0000_0000_0001,
        128,
        RefusedRange::Loopback,
    ), // ::1/128
    (
        0x0000_0000_0000_0000_0000_0000_0000_0000,
        96,
        RefusedRange::Translation,
    ), // ::/96 IPv4-compatible
    (
        0x0064_ff9b_0000_0000_0000_0000_0000_0000,
        96,
        RefusedRange::Translation,
    ), // 64:ff9b::/96 NAT64
    (
        0x0064_ff9b_0001_0000_0000_0000_0000_0000,
        48,
        RefusedRange::Translation,
    ), // 64:ff9b:1::/48
    (
        0x2002_0000_0000_0000_0000_0000_0000_0000,
        16,
        RefusedRange::Translation,
    ), // 2002::/16 6to4
    (
        0x2001_0db8_0000_0000_0000_0000_0000_0000,
        32,
        RefusedRange::Documentation,
    ), // 2001:db8::/32
    (
        0x2001_0000_0000_0000_0000_0000_0000_0000,
        32,
        RefusedRange::Translation,
    ), // 2001::/32 Teredo
    (
        0x2001_0000_0000_0000_0000_0000_0000_0000,
        23,
        RefusedRange::Reserved,
    ), // 2001::/23 IETF
    (
        0xfc00_0000_0000_0000_0000_0000_0000_0000,
        7,
        RefusedRange::UniqueLocal,
    ), // fc00::/7
    (
        0xfe80_0000_0000_0000_0000_0000_0000_0000,
        10,
        RefusedRange::LinkLocal,
    ), // fe80::/10
    (
        0xfec0_0000_0000_0000_0000_0000_0000_0000,
        10,
        RefusedRange::Private,
    ), // fec0::/10 site-local
    (
        0xff00_0000_0000_0000_0000_0000_0000_0000,
        8,
        RefusedRange::Multicast,
    ), // ff00::/8
    (
        0x0100_0000_0000_0000_0000_0000_0000_0000,
        64,
        RefusedRange::Reserved,
    ), // 100::/64 discard
];

/// Top three bits of `2000::/3` (0b001) and its network constant.
const GLOBAL_PREFIX_MASK: u128 = 0xe000_0000_0000_0000_0000_0000_0000_0000;
const GLOBAL_PREFIX_NETWORK: u128 = 0x2000_0000_0000_0000_0000_0000_0000_0000;

/// Prefix mask for an IPv4 prefix length.
fn mask_v4(prefix: u8) -> u32 {
    if prefix == 0 {
        0
    } else {
        u32::MAX << (32 - prefix)
    }
}

/// Prefix mask for an IPv6 prefix length.
fn mask_v6(prefix: u8) -> u128 {
    if prefix == 0 {
        0
    } else {
        u128::MAX << (128 - prefix)
    }
}

fn classify_v4(ip: Ipv4Addr) -> Result<(), RefusedRange> {
    let bits = u32::from(ip);
    for &(network, prefix, kind) in V4 {
        let mask = mask_v4(prefix);
        if bits & mask == network & mask {
            return Err(kind);
        }
    }
    Ok(())
}

fn classify_v6(ip: Ipv6Addr) -> Result<(), RefusedRange> {
    // IPv4-mapped IPv6: classify the embedded IPv4 (to_ipv4_mapped, which only
    // covers ::ffff:0:0/96, not the deprecated ::/96 form).
    if let Some(v4) = ip.to_ipv4_mapped() {
        return classify_v4(v4);
    }
    let bits = u128::from(ip);
    for &(network, prefix, kind) in V6 {
        let mask = mask_v6(prefix);
        if bits & mask == network & mask {
            return Err(kind);
        }
    }
    if bits & GLOBAL_PREFIX_MASK != GLOBAL_PREFIX_NETWORK {
        return Err(RefusedRange::Reserved);
    }
    Ok(())
}

/// Allowed only for globally routable unicast addresses.
pub fn classify_ip(ip: IpAddr) -> Result<(), RefusedRange> {
    match ip {
        IpAddr::V4(v4) => classify_v4(v4),
        IpAddr::V6(v6) => classify_v6(v6),
    }
}
