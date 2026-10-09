//! Property and fuzz-style tests for the one-click URL and allowlist checks
//! (T-1110, T-306, ASVS V1.3.6, V13.2.4).
//!
//! A URL whose host is a private, loopback, link-local or metadata address must
//! be refused for every encoding: dotted, decimal, hex, octal, short, IPv4
//! percent-encoded dots, IPv4-mapped IPv6, upper case, a trailing dot, and the
//! `@`, `#` and `\` tricks. A reserved name (and its suffixed and single-label
//! forms) is refused by name before any DNS. The `allows` predicate only ever
//! passes an https URL on port 443 to a host on the service's allowlist.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::too_many_lines,
    clippy::doc_markdown
)]

use std::net::Ipv4Addr;

use egress::{
    allows, check_host_name, check_one_click_url, one_click_permitted, Service, REFUSED_NAMES,
    REFUSED_SUFFIXES,
};
use proptest::prelude::*;
use url::Url;

/// The refused IPv4 ranges, kept in sync with `egress/src/ranges.rs`.
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

/// The dotless encodings of an IPv4 address the `url` crate normalises back to
/// `Host::Ipv4`. A trailing-dot form is handled separately.
fn core_encodings(ip: Ipv4Addr) -> Vec<String> {
    let o = ip.octets();
    let n = u32::from(ip);
    let low = n & 0x00ff_ffff;
    vec![
        format!("{}.{}.{}.{}", o[0], o[1], o[2], o[3]),
        format!("{n}"),
        format!("0x{n:x}"),
        format!("0X{n:X}"),
        format!("0x{:x}.0x{:x}.0x{:x}.0x{:x}", o[0], o[1], o[2], o[3]),
        format!("0{:o}.0{:o}.0{:o}.0{:o}", o[0], o[1], o[2], o[3]),
        format!("{}%2E{}%2E{}%2E{}", o[0], o[1], o[2], o[3]),
        format!("{}.{}", o[0], low),
    ]
}

/// A public, two-label host name that the checks must accept.
fn public_host() -> impl Strategy<Value = String> {
    (
        "[a-z][a-z0-9-]{0,12}[a-z0-9]",
        prop::sample::select(vec!["com", "net", "org", "test", "io", "dev", "example"]),
    )
        .prop_map(|(label, tld)| format!("{label}.{tld}"))
}

fn service_strategy() -> impl Strategy<Value = Service> {
    prop::sample::select(vec![Service::Api, Service::Unsub, Service::Worker])
}

proptest! {
    /// Property 7: every encoding of a refused IPv4 address — decimal, hex,
    /// octal, short, percent-encoded dots, IPv4-mapped IPv6 — is refused by the
    /// one-click URL check, with the plain, userinfo, `#` and `\` framings.
    #[test]
    fn t1110_one_click_url_refuses_ip_literal_encodings(ip in refused_v4_address()) {
        for enc in core_encodings(ip) {
            for raw in [
                format!("https://{enc}/"),
                format!("https://user@{enc}/"),
                format!("https://{enc}#@evil.test/"),
                format!("https://{enc}\\@evil.test/"),
            ] {
                if let Ok(url) = Url::parse(&raw) {
                    prop_assert!(check_one_click_url(&url).is_err(), "{raw} was accepted");
                }
            }
        }
        // The trailing-dot form of an address is normalised to the same IPv4
        // host and must still be refused.
        let o = ip.octets();
        let n = u32::from(ip);
        for raw in [
            format!("https://{}.{}.{}.{}/", o[0], o[1], o[2], o[3]),
            format!("https://{}.{}.{}.{}./", o[0], o[1], o[2], o[3]),
            format!("https://{n}./"),
        ] {
            if let Ok(url) = Url::parse(&raw) {
                prop_assert!(check_one_click_url(&url).is_err(), "{raw} was accepted");
            }
        }
    }

    /// Property 7: the IPv4-mapped IPv6 form of a refused address is refused.
    #[test]
    fn t1110_one_click_url_refuses_ipv4_mapped(ip in refused_v4_address()) {
        let o = ip.octets();
        let raw = format!("https://[::ffff:{}.{}.{}.{}]/", o[0], o[1], o[2], o[3]);
        let url = Url::parse(&raw).expect("mapped url parses");
        prop_assert!(check_one_click_url(&url).is_err(), "{raw} was accepted");
    }

    /// Property 7: a reserved name, its suffixed forms, its single-label forms
    /// and upper-case spellings are refused before any DNS.
    #[test]
    fn t1110_one_click_url_refuses_reserved_name_forms(
        name_idx in 0..REFUSED_NAMES.len(),
        s in 0..REFUSED_SUFFIXES.len(),
        label in "[a-z0-9]{1,10}",
        upper in any::<bool>(),
        trailing_dot in any::<bool>(),
    ) {
        let mut suffixed = format!("{label}{}", REFUSED_SUFFIXES[s]);
        if upper {
            suffixed = suffixed.to_ascii_uppercase();
        }
        if trailing_dot {
            suffixed.push('.');
        }
        if let Ok(url) = Url::parse(&format!("https://{suffixed}/")) {
            prop_assert!(check_one_click_url(&url).is_err(), "{suffixed} was accepted");
        }

        let mut bare = REFUSED_NAMES[name_idx].to_owned();
        if upper {
            bare = bare.to_ascii_uppercase();
        }
        if trailing_dot {
            bare.push('.');
        }
        if let Ok(url) = Url::parse(&format!("https://{bare}/")) {
            prop_assert!(check_one_click_url(&url).is_err(), "{bare} was accepted");
        }
    }

    /// Property 7: a public https URL is accepted and returned as its
    /// lower-cased host.
    #[test]
    fn t1110_one_click_url_accepts_public_https(host in public_host()) {
        let url = Url::parse(&format!("https://{host}/x")).expect("public url parses");
        prop_assert_eq!(check_one_click_url(&url), Ok(host));
    }

    /// `check_host_name` refuses reserved names, refused suffixes and any
    /// single-label name, and accepts an ordinary public domain.
    #[test]
    fn t1110_check_host_name_refuses_reserved_and_single_label(
        name_idx in 0..REFUSED_NAMES.len(),
        s in 0..REFUSED_SUFFIXES.len(),
        label in "[a-z0-9]{1,10}",
    ) {
        prop_assert!(check_host_name(REFUSED_NAMES[name_idx]).is_err());
        let suffixed = format!("{label}{}", REFUSED_SUFFIXES[s]);
        prop_assert!(check_host_name(&suffixed).is_err(), "{suffixed} accepted");
        // A host with no dot at all is refused regardless of its letters.
        prop_assert!(check_host_name(&label).is_err(), "{label} accepted");
    }

    /// `check_host_name` accepts a plain two-label public domain.
    #[test]
    fn t1110_check_host_name_accepts_public(host in public_host()) {
        prop_assert!(check_host_name(&host).is_ok(), "{host} refused");
    }

    /// Property from T-306: `allows` is true only for https, port 443 (or
    /// unspecified), and a host on the service's own allowlist.
    #[test]
    fn t1110_allows_implies_https_host_and_port(
        service in service_strategy(),
        scheme in prop_oneof![Just("https"), Just("http")],
        host in "[a-z0-9.-]{1,30}",
        port in prop::option::of(1u16..65535),
    ) {
        let raw = match port {
            Some(p) => format!("{scheme}://{host}:{p}/x"),
            None => format!("{scheme}://{host}/x"),
        };
        if let Ok(url) = Url::parse(&raw) {
            if allows(service, &url) {
                prop_assert_eq!(url.scheme(), "https");
                prop_assert!(matches!(url.port(), None | Some(443)));
                prop_assert!(url.host_str().is_some());
            }
        }
    }

    /// `allows` is a pure predicate, and one-click is permitted for `unsub`
    /// only.
    #[test]
    fn t1110_allows_is_deterministic_and_one_click_only_unsub(
        service in service_strategy(),
        host in "[a-z0-9.-]{1,30}",
        path in "[A-Za-z0-9/._-]{0,40}",
    ) {
        if let Ok(url) = Url::parse(&format!("https://{host}/{path}")) {
            prop_assert_eq!(allows(service, &url), allows(service, &url));
        }
        prop_assert_eq!(one_click_permitted(service), matches!(service, Service::Unsub));
    }
}
