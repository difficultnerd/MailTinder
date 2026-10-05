//! Table tests over the S10 section 5 corpus cases, driven end to end through
//! `GmailProvider` and `fake-google` (T-406).
//!
//! Each row seeds one message whose headers mirror a required S10 case, reads
//! its metadata through the public adapter, and asserts the DKIM-derived
//! [`domain::HeaderFacts`]: which unsubscribe options are offered and whether
//! the `From` is authenticated. Domains are reserved names only.
#![allow(
    clippy::pedantic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::missing_panics_doc,
    clippy::missing_errors_doc
)]

mod support;

use domain::{HeaderFacts, MessageId};
use fake_google::FakeGoogleHandle;
use ports::MailProvider;
use url::Url;

/// The `h=` tag covering both unsubscribe headers.
const H_BOTH: &str = "from:to:subject:date:list-unsubscribe:list-unsubscribe-post";
/// The `h=` tag covering `List-Unsubscribe` only.
const H_LU: &str = "from:to:subject:date:list-unsubscribe";
/// The `h=` tag covering neither unsubscribe header.
const H_NONE: &str = "from:to:subject:date";
const ONE_CLICK: &str = "List-Unsubscribe=One-Click";

fn sig(d: &str, s: &str, h: &str, b: &str) -> String {
    format!("v=1; a=rsa-sha256; c=relaxed/relaxed; d={d}; s={s}; h={h}; bh=Zmh4; b={b}")
}

/// One expected outcome: the offered URIs and the authentication verdict.
#[derive(Default)]
struct Expect {
    one_click: Option<&'static str>,
    https: Option<&'static str>,
    mailto: Option<&'static str>,
    present: bool,
    from_authenticated: bool,
}

/// A table row: a message's headers (top first) plus the expected facts.
struct Case {
    name: &'static str,
    from: &'static str,
    headers: Vec<(&'static str, String)>,
    expect: Expect,
}

/// Build the `.eml` bytes for a case: `From`, then the case headers, then a body.
fn eml(from: &str, headers: &[(&str, String)], body: &str) -> Vec<u8> {
    let mut s = String::new();
    s.push_str(&format!("From: {from}\r\n"));
    for (k, v) in headers {
        s.push_str(&format!("{k}: {v}\r\n"));
    }
    s.push_str("\r\n");
    s.push_str(body);
    s.into_bytes()
}

fn case(
    name: &'static str,
    from: &'static str,
    headers: Vec<(&'static str, String)>,
    expect: Expect,
) -> Case {
    Case {
        name,
        from,
        headers,
        expect,
    }
}

/// The S10 section 5 rows that exercise the DKIM coverage rule.
fn cases() -> Vec<Case> {
    let gmail_pass_both = "mx.google.com; dkim=pass header.i=@news.example.com header.s=sel1 header.b=AbCdEf12; spf=pass smtp.mailfrom=example.com; dmarc=pass header.from=example.com";
    vec![
        // One-click https, DKIM covers both headers, DKIM pass.
        case(
            "one-click-covered",
            "Acme News <updates@example.com>",
            vec![
                ("Authentication-Results", gmail_pass_both.to_owned()),
                ("DKIM-Signature", sig("news.example.com", "sel1", H_BOTH, "AbCdEf12zzzz")),
                ("List-Unsubscribe", "<https://u.example.com/weekly>".to_owned()),
                ("List-Unsubscribe-Post", ONE_CLICK.to_owned()),
            ],
            Expect {
                one_click: Some("https://u.example.com/weekly"),
                present: true,
                from_authenticated: true,
                ..Expect::default()
            },
        ),
        // Same, plus a mailto target; one-click is preferred and mailto is kept.
        case(
            "one-click-plus-mailto",
            "Acme News <updates@example.com>",
            vec![
                ("Authentication-Results", gmail_pass_both.to_owned()),
                ("DKIM-Signature", sig("news.example.com", "sel1", H_BOTH, "AbCdEf12zzzz")),
                (
                    "List-Unsubscribe",
                    "<https://u.example.com/weekly>, <mailto:unsub@example.com?subject=stop>".to_owned(),
                ),
                ("List-Unsubscribe-Post", ONE_CLICK.to_owned()),
            ],
            Expect {
                one_click: Some("https://u.example.com/weekly"),
                mailto: Some("unsub@example.com"),
                present: true,
                from_authenticated: true,
                ..Expect::default()
            },
        ),
        // One-click, DKIM passes only for an ESP domain, DMARC fails.
        case(
            "esp-pass-dmarc-fail",
            "Acme Deals <offers@example.com>",
            vec![
                (
                    "Authentication-Results",
                    "mx.google.com; dkim=pass header.i=@example.net header.s=sel1 header.b=AbCdEf12; dmarc=fail header.from=example.com".to_owned(),
                ),
                ("DKIM-Signature", sig("example.net", "sel1", H_BOTH, "AbCdEf12zzzz")),
                ("List-Unsubscribe", "<https://u.example.net/deals>".to_owned()),
                ("List-Unsubscribe-Post", ONE_CLICK.to_owned()),
            ],
            Expect {
                one_click: Some("https://u.example.net/deals"),
                present: true,
                from_authenticated: false,
                ..Expect::default()
            },
        ),
        // One-click headers present but not in the DKIM `h=` list.
        case(
            "one-click-not-covered",
            "Acme News <updates@example.com>",
            vec![
                ("Authentication-Results", gmail_pass_both.to_owned()),
                ("DKIM-Signature", sig("news.example.com", "sel1", H_NONE, "AbCdEf12zzzz")),
                ("List-Unsubscribe", "<https://u.example.com/weekly>".to_owned()),
                ("List-Unsubscribe-Post", ONE_CLICK.to_owned()),
            ],
            Expect {
                present: true,
                from_authenticated: true,
                ..Expect::default()
            },
        ),
        // DKIM signature fails: no options, not authenticated.
        case(
            "dkim-fail",
            "Acme News <updates@example.com>",
            vec![
                (
                    "Authentication-Results",
                    "mx.google.com; dkim=fail header.i=@news.example.com header.s=sel1 header.b=AbCdEf12; dmarc=fail header.from=example.com".to_owned(),
                ),
                ("DKIM-Signature", sig("news.example.com", "sel1", H_BOTH, "AbCdEf12zzzz")),
                ("List-Unsubscribe", "<https://u.example.com/weekly>".to_owned()),
                ("List-Unsubscribe-Post", ONE_CLICK.to_owned()),
            ],
            Expect {
                present: true,
                from_authenticated: false,
                ..Expect::default()
            },
        ),
        // mailto only, covered: a mailto job is offered.
        case(
            "mailto-covered",
            "Acme News <updates@example.com>",
            vec![
                ("Authentication-Results", gmail_pass_both.to_owned()),
                ("DKIM-Signature", sig("news.example.com", "sel1", H_LU, "AbCdEf12zzzz")),
                ("List-Unsubscribe", "<mailto:unsub@example.com?subject=stop>".to_owned()),
            ],
            Expect {
                mailto: Some("unsub@example.com"),
                present: true,
                from_authenticated: true,
                ..Expect::default()
            },
        ),
        // mailto only, unsigned (spoofed target): no options, not authenticated.
        case(
            "mailto-uncovered",
            "Acme News <updates@example.com>",
            vec![
                (
                    "Authentication-Results",
                    "mx.google.com; dkim=none header.i=@example.com; dmarc=fail header.from=example.com".to_owned(),
                ),
                ("List-Unsubscribe", "<mailto:unsub@example.net>".to_owned()),
            ],
            Expect {
                present: true,
                from_authenticated: false,
                ..Expect::default()
            },
        ),
        // https without one-click, covered: the Needs Attention link is offered.
        case(
            "https-only-covered",
            "Acme News <updates@example.com>",
            vec![
                ("Authentication-Results", gmail_pass_both.to_owned()),
                ("DKIM-Signature", sig("news.example.com", "sel1", H_LU, "AbCdEf12zzzz")),
                ("List-Unsubscribe", "<https://u.example.com/page>".to_owned()),
            ],
            Expect {
                https: Some("https://u.example.com/page"),
                present: true,
                from_authenticated: true,
                ..Expect::default()
            },
        ),
        // A plain `http://` link is dropped: no options of any kind.
        case(
            "http-plain-link",
            "Acme News <updates@example.com>",
            vec![
                ("Authentication-Results", gmail_pass_both.to_owned()),
                ("DKIM-Signature", sig("news.example.com", "sel1", H_LU, "AbCdEf12zzzz")),
                ("List-Unsubscribe", "<http://u.example.com/page>".to_owned()),
            ],
            Expect {
                present: true,
                from_authenticated: true,
                ..Expect::default()
            },
        ),
        // A sender-written `mx.google.com` pass below Gmail's own is ignored.
        case(
            "forged-lower-ar",
            "Acme News <updates@example.com>",
            vec![
                (
                    "Authentication-Results",
                    "mx.google.com; dkim=none header.i=@example.com; dmarc=fail header.from=example.com".to_owned(),
                ),
                ("DKIM-Signature", sig("news.example.com", "sel1", H_BOTH, "AbCdEf12zzzz")),
                (
                    "Authentication-Results",
                    "mx.google.com; dkim=pass header.i=@news.example.com header.s=sel1 header.b=AbCdEf12".to_owned(),
                ),
                ("List-Unsubscribe", "<https://u.example.com/weekly>".to_owned()),
                ("List-Unsubscribe-Post", ONE_CLICK.to_owned()),
            ],
            Expect {
                present: true,
                from_authenticated: false,
                ..Expect::default()
            },
        ),
        // A spoofed From: an ESP-domain pass with no DMARC pass is not the From.
        case(
            "spoofed-from",
            "Known Sender <boss@example.com>",
            vec![
                (
                    "Authentication-Results",
                    "mx.google.com; dkim=pass header.i=@example.net header.s=sel1 header.b=AbCdEf12; dmarc=fail header.from=example.com".to_owned(),
                ),
                ("DKIM-Signature", sig("example.net", "sel1", H_NONE, "AbCdEf12zzzz")),
            ],
            Expect {
                from_authenticated: false,
                ..Expect::default()
            },
        ),
    ]
}

/// Read the DKIM-derived facts for one seeded message.
async fn facts_for(handle: &FakeGoogleHandle, case: &Case) -> HeaderFacts {
    let mb = handle.add_mailbox("reader@example.org");
    let id = handle.seed_eml(
        &mb,
        &eml(case.from, &case.headers, "hello"),
        &["INBOX"],
        testkit::T0,
    );
    let ctx = support::mail_ctx(handle, &mb);
    let provider = support::provider(handle, support::clock());
    let meta = provider
        .get_meta(&ctx, &MessageId::new(id).expect("valid id"))
        .await
        .expect("metadata");
    meta.facts
}

fn as_str(url: &Option<Url>) -> Option<&str> {
    url.as_ref().map(Url::as_str)
}

#[tokio::test]
async fn dkim_coverage_matches_the_s10_corpus() {
    let handle = support::start().await;
    for case in cases() {
        let facts = facts_for(&handle, &case).await;
        let opts = facts.list_unsubscribe.as_ref();
        assert_eq!(
            facts.list_unsubscribe_present, case.expect.present,
            "{}",
            case.name
        );
        assert_eq!(
            opts.and_then(|o| as_str(&o.one_click_https)),
            case.expect.one_click,
            "one-click for {}",
            case.name
        );
        assert_eq!(
            opts.and_then(|o| as_str(&o.https)),
            case.expect.https,
            "https for {}",
            case.name
        );
        assert_eq!(
            opts.and_then(|o| o.mailto.as_ref())
                .map(domain::MailtoTarget::to),
            case.expect.mailto,
            "mailto for {}",
            case.name
        );
        assert_eq!(
            facts.from_authenticated, case.expect.from_authenticated,
            "from_authenticated for {}",
            case.name
        );
    }
}
