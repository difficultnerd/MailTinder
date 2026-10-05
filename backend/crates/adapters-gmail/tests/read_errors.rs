//! Error mapping and log-redaction tests for the Gmail read half (T-401).
//!
//! These drive the adapter through `fake-google` failure injection so the
//! egress → `map_gmail_error` path is exercised, not just the mapper.
#![allow(
    clippy::pedantic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::missing_panics_doc,
    clippy::missing_errors_doc
)]

mod support;

use domain::MailboxId;
use fake_google::FailRule;
use obs::Sensitive;
use ports::{ListOrder, MailError, MailProvider, MailboxCtx};
use time::OffsetDateTime;

/// XC-01: a read page logs no header value, address, subject or body.
#[tokio::test]
async fn xc_01_gmail_read_logs_no_header_values() {
    let handle = support::start().await;
    let mb = handle.add_mailbox("redact@example.com");
    let provider = support::provider(&handle, support::clock());
    let ctx = support::mail_ctx(&handle, &mb);

    let subject_canary = "CANARY-SUBJECT-7f21";
    let address_canary = "canary-sender@example.com";
    let list_canary = "CANARY-LISTID-9c04";
    let body_canary = "CANARY-BODY-3b8e";
    let eml = format!(
        "From: Named <{address_canary}>\r\nSubject: {subject_canary}\r\nList-Id: {list_canary}\r\n\r\n{body_canary}"
    );
    let id = handle.seed_eml(
        &mb,
        eml.as_bytes(),
        &["INBOX"],
        OffsetDateTime::from_unix_timestamp(500).unwrap(),
    );

    let (sink, _guard) = obs::capture("adapters-gmail", obs::arc(obs::FixedClock::default()));
    let page = provider
        .list_inbox(&ctx, None, ListOrder::NewestFirst)
        .await
        .unwrap();
    assert_eq!(page.items.len(), 1);
    let meta = provider
        .get_meta(&ctx, &domain::MessageId::new(&id).unwrap())
        .await
        .unwrap();
    assert!(meta.subject.contains(subject_canary));
    let _ = provider
        .get_preview(&ctx, &domain::MessageId::new(&id).unwrap())
        .await
        .unwrap();

    let text = sink.text();
    assert!(!text.is_empty(), "no log lines were captured");
    let needles = vec![
        subject_canary.to_owned(),
        address_canary.to_owned(),
        list_canary.to_owned(),
        body_canary.to_owned(),
        id.clone(),
    ];
    let leaks = obs::scan_for_leaks(&text, &needles);
    assert!(leaks.is_empty(), "log leaked: {leaks:?}");
}

/// A `429` from Gmail maps to `RateLimited` with the `Retry-After` value.
#[tokio::test]
async fn gmail_read_429_retry_after_maps_to_rate_limited() {
    let handle = support::start().await;
    let mb = handle.add_mailbox("rate@example.com");
    let provider = support::provider(&handle, support::clock());
    let ctx = support::mail_ctx(&handle, &mb);
    handle.fail(FailRule {
        method: "GET".to_owned(),
        path_prefix: "/gmail/v1/users/me/messages".to_owned(),
        status: 429,
        reason: "rateLimitExceeded".to_owned(),
        retry_after: Some("42".to_owned()),
        times: 1,
    });
    let err = provider
        .list_inbox(&ctx, None, ListOrder::NewestFirst)
        .await
        .unwrap_err();
    assert_eq!(err, MailError::RateLimited { retry_after_s: 42 });
}

/// A `500` maps to `Transient`.
#[tokio::test]
async fn gmail_read_500_maps_to_transient() {
    let handle = support::start().await;
    let mb = handle.add_mailbox("boom@example.com");
    let provider = support::provider(&handle, support::clock());
    let ctx = support::mail_ctx(&handle, &mb);
    handle.fail(FailRule {
        method: "GET".to_owned(),
        path_prefix: "/gmail/v1/users/me/messages".to_owned(),
        status: 500,
        reason: "backendError".to_owned(),
        retry_after: None,
        times: 1,
    });
    let err = provider
        .list_inbox(&ctx, None, ListOrder::NewestFirst)
        .await
        .unwrap_err();
    assert_eq!(err, MailError::Transient);
}

/// A rejected token maps to `Unauthorized`.
#[tokio::test]
async fn gmail_read_bad_token_maps_to_unauthorized() {
    let handle = support::start().await;
    let mb = handle.add_mailbox("expired@example.com");
    let provider = support::provider(&handle, support::clock());
    let ctx = MailboxCtx {
        mailbox: MailboxId(uuid::Uuid::from_u128(2)),
        access_token: Sensitive::new("tok-not-issued".to_owned()),
    };
    let err = provider
        .list_inbox(&ctx, None, ListOrder::NewestFirst)
        .await
        .unwrap_err();
    assert_eq!(err, MailError::Unauthorized);
    let _ = mb;
}

/// An egress refusal is a configuration bug: `Invalid`, never retried.
#[tokio::test]
async fn gmail_egress_refusal_is_invalid() {
    let handle = support::start().await;
    let mb = handle.add_mailbox("egress@example.com");
    let provider = support::refusing_provider(&handle);
    let ctx = support::mail_ctx(&handle, &mb);
    let err = provider
        .list_inbox(&ctx, None, ListOrder::NewestFirst)
        .await
        .unwrap_err();
    assert_eq!(err, MailError::Invalid("egress_refused".to_owned()));
}
