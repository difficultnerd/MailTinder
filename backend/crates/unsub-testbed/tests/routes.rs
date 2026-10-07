//! Integration tests of the testbed itself (S10 6.2, T-708).
//!
//! These prove the fixture, not a story AC: T-702 uses it to prove UN-02 AC1,
//! AC3 and AC4.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::too_many_lines,
    clippy::many_single_char_names,
    clippy::doc_markdown,
    clippy::ignored_unit_patterns,
    clippy::uninlined_format_args,
    clippy::similar_names,
    clippy::match_same_arms,
    clippy::single_match_else,
    clippy::redundant_clone,
    clippy::needless_pass_by_value,
    clippy::assert_is_empty,
    clippy::case_sensitive_file_extension_comparisons
)]

use std::net::IpAddr;
use std::time::Duration;

use unsub_testbed::{start, start_with_untrusted_cert, SSRF_CASES};

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap()
}

fn status_of(response: reqwest::Response) -> u16 {
    response.status().as_u16()
}

#[tokio::test]
async fn testbed_records_method_headers_body_cookies() {
    let testbed = start().await.unwrap();
    let response = client()
        .post(testbed.url("/oneclick/200"))
        .header("X-Testbed-Case", "recorded")
        .header(reqwest::header::COOKIE, "session=canary")
        .body("List-Unsubscribe=One-Click")
        .send()
        .await
        .unwrap();
    assert_eq!(status_of(response), 200);

    let records = testbed.requests_to("/oneclick/200");
    assert_eq!(records.len(), 1);
    let record = &records[0];
    assert_eq!(record.method, "POST");
    assert_eq!(record.path, "/oneclick/200");
    assert_eq!(record.body, b"List-Unsubscribe=One-Click");
    assert!(record.cookies_present);
    assert!(record
        .headers
        .iter()
        .any(|(name, value)| name == "x-testbed-case" && value == "recorded"));
    assert!(record.headers.iter().any(|(name, _)| name == "cookie"));
    // Header names are recorded lower-cased.
    assert!(record
        .headers
        .iter()
        .all(|(name, _)| !name.chars().any(|c| c.is_ascii_uppercase())));
    // The whole request is recorded, in order.
    assert_eq!(testbed.requests().len(), 1);
    testbed.shutdown().await;
}

#[tokio::test]
async fn testbed_status_routes_return_their_status() {
    let testbed = start().await.unwrap();
    let http = client();
    for (path, expected) in [
        ("/oneclick/200", 200_u16),
        ("/oneclick/202", 202),
        ("/oneclick/204", 204),
        ("/oneclick/400", 400),
        ("/oneclick/500", 500),
    ] {
        let response = http.post(testbed.url(path)).send().await.unwrap();
        assert_eq!(status_of(response), expected, "for {path}");
    }
    assert_eq!(testbed.requests_to("/oneclick/500").len(), 1);

    // A non-POST method is 405 and is still recorded.
    let response = http
        .post(testbed.url("/oneclick/200"))
        .send()
        .await
        .unwrap();
    assert_eq!(status_of(response), 200);
    let response = http.get(testbed.url("/oneclick/202")).send().await.unwrap();
    assert_eq!(status_of(response), 405);
    // Both the loop's POST and this GET were recorded; the latest is the GET.
    let records = testbed.requests_to("/oneclick/202");
    assert_eq!(records.len(), 2);
    assert_eq!(records[1].method, "GET");
    testbed.shutdown().await;
}

#[tokio::test]
async fn testbed_500_then_200_per_key() {
    let testbed = start().await.unwrap();
    let http = client();
    let alpha = testbed.url("/oneclick/500-then-200/alpha");
    let beta = testbed.url("/oneclick/500-then-200/beta");
    assert_eq!(
        status_of(http.post(alpha.clone()).send().await.unwrap()),
        500
    );
    assert_eq!(
        status_of(http.post(beta.clone()).send().await.unwrap()),
        500
    );
    // The second request for alpha succeeds; beta's first was independent.
    assert_eq!(status_of(http.post(alpha).send().await.unwrap()), 200);
    assert_eq!(status_of(http.post(beta).send().await.unwrap()), 200);
    testbed.shutdown().await;
}

#[tokio::test]
async fn testbed_fail_n_then_succeeds() {
    let testbed = start().await.unwrap();
    let http = client();
    let url = testbed.url("/oneclick/fail-n/2/twice");
    assert_eq!(status_of(http.post(url.clone()).send().await.unwrap()), 500);
    assert_eq!(status_of(http.post(url.clone()).send().await.unwrap()), 500);
    assert_eq!(status_of(http.post(url).send().await.unwrap()), 200);
    testbed.shutdown().await;
}

#[tokio::test]
async fn testbed_redirect_sets_location_and_landed_unreached() {
    let testbed = start().await.unwrap();
    let http = client(); // does not follow redirects
    for code in [301_u16, 302, 303, 307, 308] {
        let response = http
            .post(testbed.url(&format!("/oneclick/redirect/{code}")))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status().as_u16(), code);
        assert_eq!(
            response
                .headers()
                .get(reqwest::header::LOCATION)
                .map(|v| v.to_str().unwrap()),
            Some("/landed")
        );
    }
    assert!(testbed.requests_to("/landed").is_empty());
    assert_eq!(testbed.requests_to("/oneclick/redirect/302").len(), 1);
    testbed.shutdown().await;
}

#[tokio::test]
async fn testbed_hang_released_on_demand() {
    let testbed = start().await.unwrap();
    let http = client();
    let send = async {
        status_of(
            http.post(testbed.url("/oneclick/hang"))
                .send()
                .await
                .unwrap(),
        )
    };
    let release = async {
        // Release only once the request has been recorded, so the wait is real
        // (a release with no waiter is not lost: `notify_one` stores a permit).
        while testbed.requests_to("/oneclick/hang").is_empty() {
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
        testbed.release_hanging();
    };
    let (status, ()) = tokio::join!(send, release);
    assert_eq!(status, 200);
    testbed.shutdown().await;
}

#[tokio::test]
async fn testbed_https_trusted_with_test_ca() {
    let testbed = start().await.unwrap();
    let ca = reqwest::Certificate::from_pem(testbed.test_ca_pem()).unwrap();
    let https = reqwest::Client::builder()
        .add_root_certificate(ca)
        .build()
        .unwrap();
    let response = https
        .post(testbed.https_url("/oneclick/202"))
        .send()
        .await
        .unwrap();
    assert_eq!(status_of(response), 202);
    assert_eq!(testbed.requests_to("/oneclick/202").len(), 1);
    testbed.shutdown().await;
}

#[tokio::test]
async fn testbed_untrusted_cert_fails_tls() {
    let testbed = start_with_untrusted_cert().await.unwrap();
    let ca = reqwest::Certificate::from_pem(testbed.test_ca_pem()).unwrap();
    let https = reqwest::Client::builder()
        .add_root_certificate(ca)
        .build()
        .unwrap();
    let response = https.post(testbed.https_url("/oneclick/200")).send().await;
    assert!(response.is_err(), "an untrusted certificate must fail TLS");
    testbed.shutdown().await;
}

#[tokio::test]
async fn testbed_binds_port_zero_in_parallel() {
    let (first, second) = tokio::join!(start(), start());
    let (first, second) = (first.unwrap(), second.unwrap());
    assert_ne!(first.addr, second.addr);
    assert_ne!(first.https_addr, second.https_addr);
    let http = client();
    assert_eq!(
        status_of(http.post(first.url("/oneclick/200")).send().await.unwrap()),
        200
    );
    assert_eq!(
        status_of(http.post(second.url("/oneclick/200")).send().await.unwrap()),
        200
    );
    assert_eq!(first.requests_to("/oneclick/200").len(), 1);
    assert_eq!(second.requests_to("/oneclick/200").len(), 1);
    first.shutdown().await;
    second.shutdown().await;
}

/// Every range S10 6.2 names has at least one case, and the case set matches
/// the table exactly.
#[test]
fn testbed_ssrf_table_covers_s10_ranges() {
    fn answers(host: &str) -> Vec<IpAddr> {
        SSRF_CASES
            .iter()
            .find(|case| case.host == host)
            .map_or_else(Vec::new, |case| case.resolves_to.to_vec())
    }

    fn in_range(ip: IpAddr, range: &str) -> bool {
        match (ip, range) {
            (IpAddr::V4(v), "loopback4") => v.octets()[0] == 127,
            (IpAddr::V4(v), "private10") => v.octets()[0] == 10,
            (IpAddr::V4(v), "private172") => {
                v.octets()[0] == 172 && (16..32).contains(&v.octets()[1])
            }
            (IpAddr::V4(v), "private192") => v.octets()[0] == 192 && v.octets()[1] == 168,
            (IpAddr::V4(v), "cgnat") => v.octets()[0] == 100 && (64..128).contains(&v.octets()[1]),
            (IpAddr::V4(v), "linklocal") => v.octets()[0] == 169 && v.octets()[1] == 254,
            (IpAddr::V4(v), "metadata") => v.octets() == [169, 254, 169, 254],
            (IpAddr::V6(v), "loopback6") => v.segments() == [0, 0, 0, 0, 0, 0, 0, 1],
            (IpAddr::V6(v), "linklocal6") => (v.segments()[0] & 0xffc0) == 0xfe80,
            (IpAddr::V6(v), "ula6") => (v.segments()[0] & 0xfe00) == 0xfc00,
            (IpAddr::V6(v), "mapped6") => v.to_ipv4_mapped().is_some(),
            _ => false,
        }
    }

    // The exact table S10 6.2 lists.
    assert_eq!(SSRF_CASES.len(), 12);
    assert_eq!(
        answers("loopback4.test"),
        vec!["127.0.0.1".parse::<IpAddr>().unwrap()]
    );
    assert_eq!(
        answers("private10.test"),
        vec!["10.0.0.1".parse::<IpAddr>().unwrap()]
    );
    assert_eq!(
        answers("private172.test"),
        vec!["172.16.0.1".parse::<IpAddr>().unwrap()]
    );
    assert_eq!(
        answers("private192.test"),
        vec!["192.168.1.1".parse::<IpAddr>().unwrap()]
    );
    assert_eq!(
        answers("cgnat.test"),
        vec!["100.64.0.1".parse::<IpAddr>().unwrap()]
    );
    assert_eq!(
        answers("linklocal.test"),
        vec!["169.254.1.1".parse::<IpAddr>().unwrap()]
    );
    assert_eq!(
        answers("metadata-ip.test"),
        vec!["169.254.169.254".parse::<IpAddr>().unwrap()]
    );
    assert_eq!(
        answers("loopback6.test"),
        vec!["::1".parse::<IpAddr>().unwrap()]
    );
    assert_eq!(
        answers("linklocal6.test"),
        vec!["fe80::1".parse::<IpAddr>().unwrap()]
    );
    assert_eq!(
        answers("ula6.test"),
        vec!["fd00::1".parse::<IpAddr>().unwrap()]
    );
    assert_eq!(
        answers("mapped6.test"),
        vec!["::ffff:127.0.0.1".parse::<IpAddr>().unwrap()]
    );

    // `mixed.test`: two answers, one public and one private.
    let mixed = answers("mixed.test");
    assert_eq!(mixed.len(), 2);
    assert_eq!(mixed[0], "93.184.216.34".parse::<IpAddr>().unwrap());
    assert_eq!(mixed[1], "10.0.0.1".parse::<IpAddr>().unwrap());
    assert!(!in_range(mixed[0], "private10"));
    assert!(in_range(mixed[1], "private10"));

    // Every refused range S10 6.2 names is covered.
    for range in [
        "loopback4",
        "private10",
        "private172",
        "private192",
        "cgnat",
        "linklocal",
        "metadata",
        "loopback6",
        "linklocal6",
        "ula6",
        "mapped6",
    ] {
        let covered = SSRF_CASES
            .iter()
            .any(|case| case.resolves_to.iter().any(|ip| in_range(*ip, range)));
        assert!(covered, "no case covers {range}");
    }

    // Host names are reserved names only, and no case is the metadata host by
    // name (T-702 tests that one without this table).
    for case in SSRF_CASES {
        assert!(
            case.host.ends_with(".test"),
            "{} is not under .test",
            case.host
        );
        assert!(!case.note.is_empty());
        assert!(case.host != "metadata.google.internal");
    }
}
