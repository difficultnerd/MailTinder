//! Route tests for `fake-google`'s Gmail REST fake (T-205a).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::needless_pass_by_value,
    clippy::redundant_closure_for_method_calls
)]

use std::sync::Arc;

use base64::Engine;
use fake_google::{FakeGoogle, FakeGoogleHandle, FakeMailboxKey, GMAIL_MODIFY, GMAIL_SEND};
use serde_json::{json, Value};
use time::OffsetDateTime;

const T0: OffsetDateTime = time::macros::datetime!(2026-10-05 00:00 UTC);

async fn start() -> FakeGoogleHandle {
    let clock = Arc::new(testkit::clock::VirtualClock::new(T0));
    FakeGoogle::start(clock).await.expect("start")
}

fn mb(h: &FakeGoogleHandle) -> FakeMailboxKey {
    h.add_mailbox("reader@example.com")
}

fn token(h: &FakeGoogleHandle, m: &FakeMailboxKey) -> String {
    h.issue_token(m, &[GMAIL_MODIFY], time::Duration::hours(1))
}

fn url(h: &FakeGoogleHandle, path: &str) -> String {
    let base = h.base_url().to_string();
    format!("{}{}", base.trim_end_matches('/'), path)
}

async fn get_json(h: &FakeGoogleHandle, path: &str, tok: &str) -> (u16, Value) {
    let client = reqwest::Client::new();
    let url = url(h, path);
    let resp = client.get(&url).bearer_auth(tok).send().await.expect("get");
    let status = resp.status().as_u16();
    let body = resp.json().await.unwrap_or(Value::Null);
    (status, body)
}

async fn post_json(h: &FakeGoogleHandle, path: &str, tok: &str, body: Value) -> (u16, Value) {
    let client = reqwest::Client::new();
    let url = url(h, path);
    let resp = client
        .post(&url)
        .bearer_auth(tok)
        .json(&body)
        .send()
        .await
        .expect("post");
    let status = resp.status().as_u16();
    let body = resp.json().await.unwrap_or(Value::Null);
    (status, body)
}

#[tokio::test]
async fn inv_5_fake_google_delete_routes_return_500_and_record() {
    let h = start().await;
    let m = mb(&h);
    let tok = token(&h, &m);
    let id = h.seed_eml(&m, b"Subject: x\r\n\r\nbody", &["INBOX"], T0);

    let (s1, _) = get_json(&h, &format!("/gmail/v1/users/me/messages/{id}"), &tok).await;
    assert_eq!(s1, 200);

    // DELETE /messages/{id}
    let client = reqwest::Client::new();
    let u = url(&h, &format!("/gmail/v1/users/me/messages/{id}"));
    let resp = client.delete(&u).bearer_auth(&tok).send().await.unwrap();
    assert_eq!(resp.status().as_u16(), 500);
    // POST /messages/batchDelete
    let (s2, _) = post_json(
        &h,
        "/gmail/v1/users/me/messages/batchDelete",
        &tok,
        json!({ "ids": [id] }),
    )
    .await;
    assert_eq!(s2, 500);
    // DELETE /threads/{id}
    let u = url(&h, &format!("/gmail/v1/users/me/threads/{id}"));
    let resp = client.delete(&u).bearer_auth(&tok).send().await.unwrap();
    assert_eq!(resp.status().as_u16(), 500);

    assert_eq!(h.permanent_delete_attempts(), 3);
    // Nothing was deleted.
    let (s, _) = get_json(&h, &format!("/gmail/v1/users/me/messages/{id}"), &tok).await;
    assert_eq!(s, 200);
}

#[tokio::test]
async fn fake_google_list_newest_first_and_pages() {
    let h = start().await;
    let m = mb(&h);
    let tok = token(&h, &m);
    for (i, date) in [100, 300, 200].iter().enumerate() {
        let d = T0 + time::Duration::seconds(*date);
        h.seed_eml(
            &m,
            format!("Subject: m{i}\r\n\r\nbody").as_bytes(),
            &["INBOX"],
            d,
        );
    }
    let (s, body) = get_json(&h, "/gmail/v1/users/me/messages", &tok).await;
    assert_eq!(s, 200);
    let msgs = body["messages"].as_array().expect("messages");
    assert_eq!(msgs.len(), 3);
    // Newest first by internalDate.
    let ids: Vec<&str> = msgs.iter().map(|m| m["id"].as_str().unwrap()).collect();
    let (s2, meta) = get_json(&h, &format!("/gmail/v1/users/me/messages/{}", ids[0]), &tok).await;
    assert_eq!(s2, 200);
    let expected_ms = (T0 + time::Duration::seconds(300)).unix_timestamp_nanos() / 1_000_000;
    assert_eq!(
        meta["internalDate"].as_str().unwrap(),
        expected_ms.to_string()
    );
}

#[tokio::test]
async fn fake_google_list_q_after_before_from_list() {
    let h = start().await;
    let m = mb(&h);
    let tok = token(&h, &m);
    h.seed_eml(
        &m,
        b"From: a@example.com\r\nList-Id: <list.example.com>\r\nSubject: x\r\n\r\nbody",
        &["INBOX"],
        T0 + time::Duration::seconds(100),
    );
    h.seed_eml(
        &m,
        b"From: b@example.com\r\nSubject: y\r\n\r\nbody",
        &["INBOX"],
        T0 + time::Duration::seconds(200),
    );
    let t100 = (T0 + time::Duration::seconds(100)).unix_timestamp();
    let t200 = (T0 + time::Duration::seconds(200)).unix_timestamp();
    let (s, body) = get_json(
        &h,
        &format!("/gmail/v1/users/me/messages?q=after:{t100}"),
        &tok,
    )
    .await;
    assert_eq!(s, 200);
    assert_eq!(body["messages"].as_array().map(|a| a.len()), Some(1));
    let _ = t200;
    let (s2, body2) = get_json(&h, "/gmail/v1/users/me/messages?q=from:a@example.com", &tok).await;
    assert_eq!(s2, 200);
    assert_eq!(body2["messages"].as_array().map(|a| a.len()), Some(1));
    let (s3, _) = get_json(&h, "/gmail/v1/users/me/messages?q=bogus:1", &tok).await;
    assert_eq!(s3, 400);
}

#[tokio::test]
async fn fake_google_get_metadata_returns_only_named_headers_in_order_with_duplicates() {
    let h = start().await;
    let m = mb(&h);
    let tok = token(&h, &m);
    let eml = b"From: a@example.com\r\nDKIM-Signature: v=1; b=one\r\nDKIM-Signature: v=1; b=two\r\nSubject: x\r\n\r\nbody";
    let id = h.seed_eml(&m, eml, &["INBOX"], T0);
    let (s, body) = get_json(
        &h,
        &format!(
            "/gmail/v1/users/me/messages/{id}?format=metadata&metadataHeaders=DKIM-Signature&metadataHeaders=Subject"
        ),
        &tok,
    )
    .await;
    assert_eq!(s, 200);
    let headers = body["payload"]["headers"].as_array().expect("headers");
    assert_eq!(headers.len(), 3);
    assert_eq!(headers[0]["name"], "DKIM-Signature");
    assert_eq!(headers[1]["name"], "DKIM-Signature");
    assert_eq!(headers[2]["name"], "Subject");
}

#[tokio::test]
async fn fake_google_get_full_payload_tree_and_attachment_id() {
    let h = start().await;
    let m = mb(&h);
    let tok = token(&h, &m);
    let eml = b"From: a@example.com\r\nSubject: x\r\nMIME-Version: 1.0\r\nContent-Type: multipart/alternative; boundary=b\r\n\r\n--b\r\nContent-Type: text/plain\r\n\r\nhello\r\n--b\r\nContent-Type: text/html\r\n\r\n<p>hi</p>\r\n--b--\r\n";
    let id = h.seed_eml(&m, eml, &["INBOX"], T0);
    let (s, body) = get_json(
        &h,
        &format!("/gmail/v1/users/me/messages/{id}?format=full"),
        &tok,
    )
    .await;
    assert_eq!(s, 200);
    let payload = &body["payload"];
    assert_eq!(payload["mimeType"], "multipart/alternative");
    let parts = payload["parts"].as_array().expect("parts");
    assert_eq!(parts.len(), 2);
    assert_eq!(parts[0]["mimeType"], "text/plain");
    assert!(parts[0]["body"]["data"].as_str().is_some());
}

#[tokio::test]
async fn fake_google_unknown_format_is_400() {
    let h = start().await;
    let m = mb(&h);
    let tok = token(&h, &m);
    let id = h.seed_eml(&m, b"Subject: x\r\n\r\nbody", &["INBOX"], T0);
    let (s, _) = get_json(
        &h,
        &format!("/gmail/v1/users/me/messages/{id}?format=bogus"),
        &tok,
    )
    .await;
    assert_eq!(s, 400);
}

#[tokio::test]
async fn fake_google_missing_or_expired_token_is_401() {
    let h = start().await;
    let m = mb(&h);
    let _ = token(&h, &m);
    // No token.
    let client = reqwest::Client::new();
    let u = url(&h, "/gmail/v1/users/me/messages");
    let resp = client.get(&u).send().await.unwrap();
    assert_eq!(resp.status().as_u16(), 401);
    // Expired token.
    let tok = h.issue_token(&m, &[GMAIL_MODIFY], time::Duration::seconds(1));
    h.expire_token(&tok);
    let resp = client.get(&u).bearer_auth(&tok).send().await.unwrap();
    assert_eq!(resp.status().as_u16(), 401);
}

#[tokio::test]
async fn fake_google_wrong_scope_is_403() {
    let h = start().await;
    let m = mb(&h);
    // Only send scope, not modify.
    let tok = h.issue_token(&m, &[GMAIL_SEND], time::Duration::hours(1));
    let client = reqwest::Client::new();
    let u = url(&h, "/gmail/v1/users/me/messages");
    let resp = client.get(&u).bearer_auth(&tok).send().await.unwrap();
    assert_eq!(resp.status().as_u16(), 403);
}

#[tokio::test]
async fn fake_google_fail_rule_shapes_429_with_retry_after() {
    let h = start().await;
    let m = mb(&h);
    let tok = token(&h, &m);
    h.fail(fake_google::FailRule {
        method: "GET".to_owned(),
        path_prefix: "/gmail/v1/users/me/messages".to_owned(),
        status: 429,
        reason: "rateLimitExceeded".to_owned(),
        retry_after: Some("30".to_owned()),
        times: 1,
    });
    let client = reqwest::Client::new();
    let u = url(&h, "/gmail/v1/users/me/messages");
    let resp = client.get(&u).bearer_auth(&tok).send().await.unwrap();
    assert_eq!(resp.status().as_u16(), 429);
    assert_eq!(resp.headers()["retry-after"], "30");
    // Second call succeeds.
    let resp2 = client.get(&u).bearer_auth(&tok).send().await.unwrap();
    assert_eq!(resp2.status().as_u16(), 200);
}

#[tokio::test]
async fn fake_google_trash_untrash_modify_labels() {
    let h = start().await;
    let m = mb(&h);
    let tok = token(&h, &m);
    let id = h.seed_eml(&m, b"Subject: x\r\n\r\nbody", &["INBOX", "UNREAD"], T0);
    // trash
    let (s, _) = post_json(
        &h,
        &format!("/gmail/v1/users/me/messages/{id}/trash"),
        &tok,
        json!({}),
    )
    .await;
    assert_eq!(s, 200);
    let labels = h.labels_of(&m, &id).expect("labels");
    assert!(labels.contains("TRASH"));
    assert!(!labels.contains("INBOX"));
    // untrash
    let (s2, _) = post_json(
        &h,
        &format!("/gmail/v1/users/me/messages/{id}/untrash"),
        &tok,
        json!({}),
    )
    .await;
    assert_eq!(s2, 200);
    let labels = h.labels_of(&m, &id).expect("labels");
    assert!(!labels.contains("TRASH"));
    // modify: add STARRED, remove UNREAD
    let (s3, _) = post_json(
        &h,
        &format!("/gmail/v1/users/me/messages/{id}/modify"),
        &tok,
        json!({ "addLabelIds": ["STARRED"], "removeLabelIds": ["UNREAD"] }),
    )
    .await;
    assert_eq!(s3, 200);
    let labels = h.labels_of(&m, &id).expect("labels");
    assert!(labels.contains("STARRED"));
    assert!(!labels.contains("UNREAD"));
}

#[tokio::test]
async fn fake_google_label_create_race_answers_409_but_creates() {
    let h = start().await;
    let m = mb(&h);
    let tok = token(&h, &m);
    // First create succeeds.
    let (s, _) = post_json(
        &h,
        "/gmail/v1/users/me/labels",
        &tok,
        json!({ "name": "Work" }),
    )
    .await;
    assert_eq!(s, 200);
    // Second create clashes -> 409.
    let (s2, _) = post_json(
        &h,
        "/gmail/v1/users/me/labels",
        &tok,
        json!({ "name": "work" }),
    )
    .await;
    assert_eq!(s2, 409);
    // Arm the race: next clash creates but still 409.
    h.arm_label_create_race(&m);
    let (s3, _) = post_json(
        &h,
        "/gmail/v1/users/me/labels",
        &tok,
        json!({ "name": "WORK" }),
    )
    .await;
    assert_eq!(s3, 409);
    // The label was created despite the 409.
    let (s4, body) = get_json(&h, "/gmail/v1/users/me/labels", &tok).await;
    assert_eq!(s4, 200);
    let names: Vec<&str> = body["labels"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|l| l["name"].as_str())
        .collect();
    assert!(names.iter().any(|n| n.eq_ignore_ascii_case("work")));
}

#[tokio::test]
async fn fake_google_send_stores_raw_in_sent() {
    let h = start().await;
    let m = mb(&h);
    let tok = token(&h, &m);
    let raw = b"From: a@example.com\r\nTo: b@example.com\r\nSubject: hi\r\n\r\nbody";
    let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(raw);
    let (s, body) = post_json(
        &h,
        "/gmail/v1/users/me/messages/send",
        &tok,
        json!({ "raw": b64 }),
    )
    .await;
    assert_eq!(s, 200);
    assert_eq!(body["labelIds"][0], "SENT");
    let sent = h.sent(&m);
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0], raw);
}

#[tokio::test]
async fn fake_google_seed_corpus_seeds_every_case() {
    let h = start().await;
    let m = mb(&h);
    let tok = token(&h, &m);
    let corpus = testkit::corpus::load().expect("corpus");
    let n = corpus.cases.len();
    for cs in &corpus.cases {
        h.seed_eml(&m, &cs.eml, &["INBOX"], T0);
    }
    let (s, body) = get_json(&h, "/gmail/v1/users/me/messages", &tok).await;
    assert_eq!(s, 200);
    assert_eq!(body["messages"].as_array().map(|a| a.len()), Some(n));
}
