//! Contract tests for the Gmail send half against `fake-google` (T-404).
//!
//! Every send goes through the `MailProvider` or `InviteMailer` trait; the fake
//! stores the raw message it received so the tests can decode it and assert the
//! exact `From`, `To`, `Subject` and body, and that nothing is ever deleted.
#![allow(
    clippy::pedantic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::missing_panics_doc,
    clippy::missing_errors_doc
)]

mod support;

use std::sync::atomic::{AtomicU32, Ordering};

use adapters_gmail::{INVITE_BODY_TEMPLATE, INVITE_SUBJECT};
use base64::Engine as _;
use domain::{EmailAddress, MailtoTarget};
use fake_google::{FailRule, FakeEvent, FakeGoogleHandle, FakeMailboxKey};
use obs::Sensitive;
use ports::{InviteLink, InviteMailer, MailError, MailProvider};
use testkit::contract::mail_provider::{mail_provider, CaseGroups};
use testkit::FakeInviteMailer;
use url::Url;

/// The value of the first `name` header in a raw message.
fn header(raw: &[u8], name: &str) -> Option<String> {
    let text = String::from_utf8_lossy(raw);
    text.split("\r\n")
        .take_while(|line| !line.is_empty())
        .find_map(|line| {
            let (key, value) = line.split_once(':')?;
            key.eq_ignore_ascii_case(name)
                .then(|| value.trim().to_owned())
        })
}

/// The decoded base64 body of a raw message.
fn body_text(raw: &[u8]) -> String {
    let text = String::from_utf8_lossy(raw);
    let encoded = text.split_once("\r\n\r\n").map_or("", |(_, body)| body);
    let compact: String = encoded.chars().filter(|c| !c.is_whitespace()).collect();
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(compact)
        .expect("base64 body");
    String::from_utf8(bytes).expect("utf-8 body")
}

/// The one raw message the fake recorded as sent.
fn first_sent(handle: &FakeGoogleHandle, mb: &FakeMailboxKey) -> Vec<u8> {
    handle.sent(mb).into_iter().next().expect("a sent message")
}

/// The number of `POST /messages/send` requests the fake has seen.
fn send_events(handle: &FakeGoogleHandle) -> usize {
    handle
        .events()
        .into_iter()
        .filter(|e| {
            matches!(
                e,
                FakeEvent::Request { method, route, .. }
                    if route == "messages.send" && method == "POST"
            )
        })
        .count()
}

/// A parsed target from a URI.
fn target(uri: &str) -> MailtoTarget {
    MailtoTarget::parse(uri).expect("valid mailto")
}

/// Make the next `messages.send` fail with `status`.
fn fail_next_send(handle: &FakeGoogleHandle, status: u16, reason: &str, retry_after: Option<&str>) {
    handle.fail(FailRule {
        method: "POST".to_owned(),
        path_prefix: "/gmail/v1/users/me/messages/send".to_owned(),
        status,
        reason: reason.to_owned(),
        retry_after: retry_after.map(str::to_owned),
        times: 1,
    });
}

/// UN-03 AC1: the send uses the address, subject and body in the URI, from the
/// mailbox's own address.
#[tokio::test]
async fn un_03_ac1_send_uses_uri_address_subject_body() {
    let handle = support::start().await;
    let mb = handle.add_mailbox("sender@example.com");
    let provider = support::provider(&handle, support::clock());
    let ctx = support::mail_ctx(&handle, &mb);

    provider
        .send_mailto(
            &ctx,
            &target("mailto:dest@example.org?subject=Unsubscribe&body=Please%20stop"),
        )
        .await
        .expect("send");

    let raw = first_sent(&handle, &mb);
    assert_eq!(header(&raw, "From").as_deref(), Some("sender@example.com"));
    assert_eq!(header(&raw, "To").as_deref(), Some("dest@example.org"));
    assert_eq!(header(&raw, "Subject").as_deref(), Some("Unsubscribe"));
    assert_eq!(body_text(&raw), "Please stop");
    assert_eq!(handle.permanent_delete_attempts(), 0);
}

/// UN-03 AC1: an absent subject or body sends the default text.
#[tokio::test]
async fn un_03_ac1_default_subject_and_body_when_absent() {
    let handle = support::start().await;
    let mb = handle.add_mailbox("default@example.com");
    let provider = support::provider(&handle, support::clock());
    let ctx = support::mail_ctx(&handle, &mb);

    provider
        .send_mailto(&ctx, &target("mailto:dest@example.org"))
        .await
        .expect("send");

    let raw = first_sent(&handle, &mb);
    assert_eq!(header(&raw, "Subject").as_deref(), Some("unsubscribe"));
    assert_eq!(body_text(&raw), "unsubscribe");
}

/// UN-03 AC2: the sent message is left in Sent with the "Mail Tinder" label.
#[tokio::test]
async fn un_03_ac2_sent_message_gets_mail_tinder_label() {
    let handle = support::start().await;
    let mb = handle.add_mailbox("labelled@example.com");
    let provider = support::provider(&handle, support::clock());
    let ctx = support::mail_ctx(&handle, &mb);

    provider
        .send_mailto(&ctx, &target("mailto:dest@example.org?subject=bye"))
        .await
        .expect("send");

    let label = provider
        .ensure_label(&ctx, "Mail Tinder")
        .await
        .expect("label id");
    let labelled = handle
        .message_labels(&mb)
        .into_iter()
        .any(|(_, labels)| labels.contains(&label));
    assert!(
        labelled,
        "the sent message must carry the Mail Tinder label"
    );
    assert_eq!(handle.permanent_delete_attempts(), 0);
}

/// UN-03 AC2: a labelling failure still returns `Ok` and sends exactly once.
#[tokio::test]
async fn un_03_ac2_label_failure_still_returns_ok_and_sends_once() {
    let handle = support::start().await;
    let mb = handle.add_mailbox("labelfail@example.com");
    let provider = support::provider(&handle, support::clock());
    let ctx = support::mail_ctx(&handle, &mb);

    handle.fail(FailRule {
        method: "POST".to_owned(),
        path_prefix: "/gmail/v1/users/me/labels".to_owned(),
        status: 500,
        reason: "backendError".to_owned(),
        retry_after: None,
        times: 1,
    });

    provider
        .send_mailto(&ctx, &target("mailto:dest@example.org"))
        .await
        .expect("a failed label must not fail the send");

    assert_eq!(send_events(&handle), 1, "the message must be sent once");
    assert_eq!(handle.sent(&mb).len(), 1);
}

/// AU-01 AC1: the invite email carries the sign-in link from the admin's
/// mailbox, with the fixed subject and body.
#[tokio::test]
async fn au_01_ac1_invite_email_sent_with_link() {
    let handle = support::start().await;
    let mb = handle.add_mailbox("admin@example.com");
    let provider = support::provider(&handle, support::clock());
    let ctx = support::mail_ctx(&handle, &mb);

    let link_str = "https://app.example.com/#/invite?t=abc123";
    let link = InviteLink(Sensitive::new(Url::parse(link_str).expect("url")));
    let to = EmailAddress::parse("invitee@example.org").expect("address");
    provider
        .send_invite(&ctx, &to, &link)
        .await
        .expect("send invite");

    let raw = first_sent(&handle, &mb);
    assert_eq!(header(&raw, "From").as_deref(), Some("admin@example.com"));
    assert_eq!(header(&raw, "To").as_deref(), Some("invitee@example.org"));
    assert_eq!(header(&raw, "Subject").as_deref(), Some(INVITE_SUBJECT));
    assert_eq!(
        body_text(&raw),
        INVITE_BODY_TEMPLATE.replace("{link}", link_str)
    );

    // A non-`https` link is refused before any send happens.
    let bad = InviteLink(Sensitive::new(
        Url::parse("http://app.example.com/#/invite?t=abc123").expect("url"),
    ));
    assert_eq!(
        provider.send_invite(&ctx, &to, &bad).await,
        Err(MailError::Invalid("invite_link".to_owned()))
    );
    assert_eq!(handle.sent(&mb).len(), 1);
}

/// A rate-limited send maps to `RateLimited` and is never retried.
#[tokio::test]
async fn gmail_send_rate_limited_maps_and_does_not_retry() {
    let handle = support::start().await;
    let mb = handle.add_mailbox("ratelimited@example.com");
    let provider = support::provider(&handle, support::clock());
    let ctx = support::mail_ctx(&handle, &mb);

    fail_next_send(&handle, 429, "rateLimitExceeded", Some("30"));

    let result = provider
        .send_mailto(&ctx, &target("mailto:dest@example.org"))
        .await;

    assert_eq!(result, Err(MailError::RateLimited { retry_after_s: 30 }));
    assert_eq!(send_events(&handle), 1, "the send must be attempted once");
    assert!(handle.sent(&mb).is_empty(), "no message may be sent");
}

/// The shared `MailProvider` contract suite, send group, against `fake-google`.
#[tokio::test]
async fn mail_provider_contract_send_gmail() -> Result<(), String> {
    let handle = support::start().await;
    let counter = AtomicU32::new(0);
    let make = || {
        let email = format!("send{}@example.com", counter.fetch_add(1, Ordering::SeqCst));
        let target = support::target(&handle, &email);
        async move { target }
    };
    mail_provider(
        make,
        CaseGroups {
            read: false,
            modify: false,
            send: true,
        },
    )
    .await?;
    if handle.permanent_delete_attempts() != 0 {
        return Err("the suite attempted a permanent delete".into());
    }
    Ok(())
}

/// The recording `InviteMailer` fake captures the recipient and the link.
#[tokio::test]
async fn fake_invite_mailer_records_to_and_link() {
    let mailer = FakeInviteMailer::new();
    let mb = ports::MailboxCtx {
        mailbox: domain::MailboxId(uuid::Uuid::from_u128(1)),
        access_token: Sensitive::new("token".to_owned()),
    };
    let to = EmailAddress::parse("invitee@example.org").expect("address");
    let link = InviteLink(Sensitive::new(
        Url::parse("https://app.example.com/#/invite?t=xyz").expect("url"),
    ));
    mailer
        .send_invite(&mb, &to, &link)
        .await
        .expect("send invite");

    let sent = mailer.sent();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].0, to);
    assert_eq!(sent[0].1, "https://app.example.com/#/invite?t=xyz");
}
