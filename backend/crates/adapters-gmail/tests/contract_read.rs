//! Contract tests for the Gmail read half against `fake-google` (T-401).
//!
//! Every call goes through the trait; the fake records the requests so the
//! tests can assert the exact query the adapter sent and that no attachment
//! fetch ever leaves the adapter.
#![allow(
    clippy::pedantic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::missing_panics_doc,
    clippy::missing_errors_doc
)]

mod support;

use std::sync::atomic::{AtomicU32, Ordering};

use domain::{LabelSet, MessageId};
use fake_google::{FailRule, FakeEvent, FakeGoogleHandle};
use ports::{ListOrder, MailProvider, MessagePage};
use testkit::contract::mail_provider::{mail_provider, CaseGroups};
use time::OffsetDateTime;

/// Build a plain-text `.eml`.
fn eml(from: &str, subject: &str, extra_headers: &[(&str, &str)], body: &str) -> Vec<u8> {
    let mut s = String::new();
    s.push_str(&format!("From: {from}\r\n"));
    s.push_str(&format!("Subject: {subject}\r\n"));
    for (k, v) in extra_headers {
        s.push_str(&format!("{k}: {v}\r\n"));
    }
    s.push_str("\r\n");
    s.push_str(body);
    s.into_bytes()
}

/// A multipart `.eml` with a text/plain, a text/html and an attachment part.
fn multipart_eml() -> Vec<u8> {
    let s = concat!(
        "From: multi@example.com\r\n",
        "Subject: multi\r\n",
        "MIME-Version: 1.0\r\n",
        "Content-Type: multipart/mixed; boundary=\"B\"\r\n",
        "\r\n",
        "--B\r\n",
        "Content-Type: multipart/alternative; boundary=\"A\"\r\n",
        "\r\n",
        "--A\r\n",
        "Content-Type: text/plain; charset=utf-8\r\n",
        "\r\n",
        "PLAIN-CANARY-BODY\r\n",
        "--A\r\n",
        "Content-Type: text/html; charset=utf-8\r\n",
        "\r\n",
        "<p>HTML-CANARY-BODY</p>\r\n",
        "--A--\r\n",
        "--B\r\n",
        "Content-Type: application/octet-stream; name=\"x.bin\"\r\n",
        "Content-Disposition: attachment; filename=\"x.bin\"\r\n",
        "\r\n",
        "ATTACH-CANARY-BYTES\r\n",
        "--B--\r\n",
    );
    s.as_bytes().to_vec()
}

/// The `q` values recorded for `messages.list`.
fn list_queries(handle: &FakeGoogleHandle) -> Vec<Vec<(String, String)>> {
    handle
        .events()
        .into_iter()
        .filter_map(|e| match e {
            FakeEvent::Request {
                route,
                query,
                method,
                ..
            } if route == "messages.list" && method == "GET" => Some(query),
            _ => None,
        })
        .collect()
}

/// The routes recorded for `messages.get`.
fn get_routes(handle: &FakeGoogleHandle) -> Vec<String> {
    handle
        .events()
        .into_iter()
        .filter_map(|e| match e {
            FakeEvent::Request { route, .. } if route == "messages.get" => Some(route),
            _ => None,
        })
        .collect()
}

/// The full read case set of the shared `MailProvider` contract suite.
#[tokio::test]
async fn mail_provider_contract_list_and_meta_gmail() -> Result<(), String> {
    let handle = support::start().await;
    let counter = AtomicU32::new(0);
    let make = || {
        let email = format!(
            "contract{}@example.com",
            counter.fetch_add(1, Ordering::SeqCst)
        );
        let target = support::target(&handle, &email);
        async move { target }
    };
    mail_provider(
        make,
        CaseGroups {
            read: true,
            modify: false,
            send: false,
        },
    )
    .await
}

/// FD-04 AC1: a message that disappears between list and get is skipped
/// silently, not an error.
#[tokio::test]
async fn fd_04_ac1_message_gone_between_list_and_get_is_skipped() {
    let handle = support::start().await;
    let mb = handle.add_mailbox("gone@example.com");
    let provider = support::provider(&handle, support::clock());
    let ctx = support::mail_ctx(&handle, &mb);
    let date = OffsetDateTime::from_unix_timestamp(500).unwrap();
    handle.seed_eml(
        &mb,
        &eml("a@example.com", "first", &[], "one"),
        &["INBOX"],
        date,
    );
    handle.seed_eml(
        &mb,
        &eml("b@example.com", "second", &[], "two"),
        &["INBOX"],
        date - time::Duration::seconds(10),
    );
    // The next single `messages.get` answers 404, as if the message left.
    handle.fail(FailRule {
        method: "GET".to_owned(),
        path_prefix: "/gmail/v1/users/me/messages/".to_owned(),
        status: 404,
        reason: "notFound".to_owned(),
        retry_after: None,
        times: 1,
    });
    let page: MessagePage = provider
        .list_inbox(&ctx, None, ListOrder::NewestFirst)
        .await
        .expect("a gone message must not fail the page");
    assert_eq!(page.items.len(), 1);
}

/// Gmail always returns newest first; the adapter keeps list order.
#[tokio::test]
async fn gmail_list_newer_than_sends_after_query() {
    let handle = support::start().await;
    let mb = handle.add_mailbox("after@example.com");
    let provider = support::provider(&handle, support::clock());
    let ctx = support::mail_ctx(&handle, &mb);
    handle.seed_eml(
        &mb,
        &eml("a@example.com", "later", &[], "body"),
        &["INBOX"],
        OffsetDateTime::from_unix_timestamp(500).unwrap(),
    );
    let page = provider
        .list_inbox(
            &ctx,
            None,
            ListOrder::NewerThan(OffsetDateTime::from_unix_timestamp(100).unwrap()),
        )
        .await
        .unwrap();
    assert_eq!(page.items.len(), 1);
    let queries = list_queries(&handle);
    assert!(
        queries
            .iter()
            .any(|q| q.contains(&("q".to_owned(), "after:100".to_owned()))),
        "no after: query recorded: {queries:?}"
    );
}

/// Headers come back in message order: with duplicates, the first wins.
#[tokio::test]
async fn gmail_meta_reads_headers_in_message_order() {
    let handle = support::start().await;
    let mb = handle.add_mailbox("order@example.com");
    let provider = support::provider(&handle, support::clock());
    let ctx = support::mail_ctx(&handle, &mb);
    let id = handle.seed_eml(
        &mb,
        &eml(
            "first@example.com",
            "ordered",
            &[
                ("From", "second@example.com"),
                ("List-Id", "First <list.one.example>"),
                ("List-Id", "Second <list.two.example>"),
                ("Authentication-Results", "first-result"),
                ("Authentication-Results", "second-result"),
            ],
            "body",
        ),
        &["INBOX"],
        OffsetDateTime::from_unix_timestamp(500).unwrap(),
    );
    let meta = provider
        .get_meta(&ctx, &MessageId::new(id).unwrap())
        .await
        .unwrap();
    assert_eq!(meta.from_address, "first@example.com");
    assert_eq!(meta.facts.list_id.as_deref(), Some("list.one.example"));
    assert_eq!(get_routes(&handle).len(), 1);
}

/// The label IDs come back exactly as Gmail gave them.
#[tokio::test]
async fn gmail_meta_label_ids_exact() {
    let handle = support::start().await;
    let mb = handle.add_mailbox("labels@example.com");
    let provider = support::provider(&handle, support::clock());
    let ctx = support::mail_ctx(&handle, &mb);
    let id = handle.seed_eml(
        &mb,
        &eml("a@example.com", "labelled", &[], "body"),
        &["INBOX", "UNREAD", "CATEGORY_PROMOTIONS"],
        OffsetDateTime::from_unix_timestamp(500).unwrap(),
    );
    let meta = provider
        .get_meta(&ctx, &MessageId::new(id).unwrap())
        .await
        .unwrap();
    let expected = LabelSet::from_ids(
        ["INBOX", "UNREAD", "CATEGORY_PROMOTIONS"]
            .into_iter()
            .map(str::to_owned),
    );
    assert_eq!(meta.labels, expected);
}

/// The preview prefers the text/plain part and never fetches an attachment.
#[tokio::test]
async fn gmail_preview_prefers_text_plain_and_never_fetches_attachment() {
    let handle = support::start().await;
    let mb = handle.add_mailbox("preview@example.com");
    let provider = support::provider(&handle, support::clock());
    let ctx = support::mail_ctx(&handle, &mb);
    let id = handle.seed_eml(
        &mb,
        &multipart_eml(),
        &["INBOX"],
        OffsetDateTime::from_unix_timestamp(500).unwrap(),
    );
    let preview = provider
        .get_preview(&ctx, &MessageId::new(id).unwrap())
        .await
        .unwrap();
    assert_eq!(preview.trim(), "PLAIN-CANARY-BODY");
    for event in handle.events() {
        if let FakeEvent::Request { route, .. } = event {
            assert!(
                !route.contains("attachment"),
                "an attachment was fetched: {route}"
            );
        }
    }
}

/// A text/html body is reduced to plain text (T-402).
#[tokio::test]
async fn gmail_preview_html_part_is_stripped() {
    let handle = support::start().await;
    let mb = handle.add_mailbox("html@example.com");
    let provider = support::provider(&handle, support::clock());
    let ctx = support::mail_ctx(&handle, &mb);
    let id = handle.seed_eml(
        &mb,
        &eml(
            "a@example.com",
            "html",
            &[("Content-Type", "text/html; charset=utf-8")],
            "<p>HTML-CANARY-BODY</p>",
        ),
        &["INBOX"],
        OffsetDateTime::from_unix_timestamp(500).unwrap(),
    );
    let preview = provider
        .get_preview(&ctx, &MessageId::new(id).unwrap())
        .await
        .unwrap();
    assert_eq!(preview.trim(), "HTML-CANARY-BODY");
}

/// `inbox_count` reads `messagesTotal` for the INBOX label.
#[tokio::test]
async fn gmail_inbox_count_reads_label_total() {
    let handle = support::start().await;
    let mb = handle.add_mailbox("count@example.com");
    let provider = support::provider(&handle, support::clock());
    let ctx = support::mail_ctx(&handle, &mb);
    for n in 0..3 {
        handle.seed_eml(
            &mb,
            &eml("a@example.com", "n", &[], "body"),
            &["INBOX"],
            OffsetDateTime::from_unix_timestamp(100 + n).unwrap(),
        );
    }
    assert_eq!(provider.inbox_count(&ctx).await.unwrap(), 3);
}
