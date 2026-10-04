//! Drive appDataFolder route tests for `fake-google` (T-205b).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::needless_pass_by_value,
    clippy::redundant_closure_for_method_calls
)]

use std::sync::Arc;

use fake_google::{FakeGoogle, FakeGoogleHandle, FakeMailboxKey, DRIVE_APPDATA, GMAIL_MODIFY};
use time::OffsetDateTime;

const T0: OffsetDateTime = time::macros::datetime!(2026-10-05 00:00 UTC);

async fn start() -> FakeGoogleHandle {
    let clock = Arc::new(testkit::clock::VirtualClock::new(T0));
    FakeGoogle::start(clock).await.expect("start")
}

fn setup(h: &FakeGoogleHandle) -> (FakeMailboxKey, String) {
    let mb = h.add_mailbox("alice@example.com");
    let token = h.issue_token(&mb, &[DRIVE_APPDATA], time::Duration::hours(1));
    (mb, token)
}

fn base(h: &FakeGoogleHandle) -> String {
    h.base_url().to_string().trim_end_matches('/').to_owned()
}

fn bearer(token: &str) -> String {
    format!("Bearer {token}")
}

/// Build a `multipart/related` body with a JSON metadata part and a bytes part.
fn multipart(boundary: &str, name: &str, parents: &str, data: &[u8]) -> Vec<u8> {
    let meta = format!("{{\"name\":\"{name}\",\"parents\":[{parents}]}}");
    let mut body = Vec::new();
    body.extend_from_slice(
        format!("--{boundary}\r\nContent-Type: application/json; charset=UTF-8\r\n\r\n{meta}\r\n")
            .as_bytes(),
    );
    body.extend_from_slice(
        format!("--{boundary}\r\nContent-Type: application/octet-stream\r\n\r\n").as_bytes(),
    );
    body.extend_from_slice(data);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    body
}

#[tokio::test]
async fn fake_drive_create_list_get_media_round_trip() {
    let h = start().await;
    let (mb, token) = setup(&h);
    let url = format!("{}/upload/drive/v3/files?uploadType=multipart", base(&h));
    let boundary = "bnd123";
    let body = multipart(boundary, "app.json", "\"appDataFolder\"", b"hello drive");
    let resp = reqwest::Client::new()
        .post(&url)
        .header("Authorization", bearer(&token))
        .header(
            "Content-Type",
            format!("multipart/related; boundary={boundary}"),
        )
        .body(body)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "create status");
    let created: serde_json::Value = resp.json().await.unwrap();
    let id = created["id"].as_str().unwrap().to_owned();
    assert!(id.starts_with("appdata-"));

    // List requires spaces=appDataFolder.
    let list_url = format!("{}/drive/v3/files?spaces=appDataFolder", base(&h));
    let list: serde_json::Value = reqwest::Client::new()
        .get(&list_url)
        .header("Authorization", bearer(&token))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(list["files"][0]["name"], "app.json");

    // Get media returns the raw bytes.
    let media_url = format!("{}/drive/v3/files/{id}?alt=media", base(&h));
    let media = reqwest::Client::new()
        .get(&media_url)
        .header("Authorization", bearer(&token))
        .send()
        .await
        .unwrap();
    assert_eq!(media.status(), 200);
    assert_eq!(media.bytes().await.unwrap().as_ref(), b"hello drive");

    // Handle exposes the stored bytes.
    assert_eq!(h.drive_file_bytes(&mb, &id).unwrap(), b"hello drive");
    assert_eq!(h.drive_files(&mb), vec![(id, "app.json".to_owned(), 11)]);
}

#[tokio::test]
async fn fake_drive_update_with_current_etag_changes_etag() {
    let h = start().await;
    let (mb, token) = setup(&h);
    let url = format!("{}/upload/drive/v3/files?uploadType=multipart", base(&h));
    let boundary = "b1";
    let body = multipart(boundary, "f.json", "\"appDataFolder\"", b"v1");
    let created: serde_json::Value = reqwest::Client::new()
        .post(&url)
        .header("Authorization", bearer(&token))
        .header(
            "Content-Type",
            format!("multipart/related; boundary={boundary}"),
        )
        .body(body)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let id = created["id"].as_str().unwrap().to_owned();

    // v2 metadata gives the etag.
    let meta_url = format!("{}/drive/v2/files/{id}", base(&h));
    let meta: serde_json::Value = reqwest::Client::new()
        .get(&meta_url)
        .header("Authorization", bearer(&token))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let etag = meta["etag"].as_str().unwrap().to_owned();
    assert_eq!(etag, "\"v1\"");

    // Update with the current etag.
    let upd_url = format!("{}/upload/drive/v2/files/{id}?uploadType=media", base(&h));
    let upd: serde_json::Value = reqwest::Client::new()
        .put(&upd_url)
        .header("Authorization", bearer(&token))
        .header("If-Match", &etag)
        .body("v2")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(upd["etag"], "\"v2\"");
    assert_eq!(h.drive_file_bytes(&mb, &id).unwrap(), b"v2");
}

#[tokio::test]
async fn fake_drive_update_with_stale_etag_is_412() {
    let h = start().await;
    let (_, token) = setup(&h);
    let url = format!("{}/upload/drive/v3/files?uploadType=multipart", base(&h));
    let boundary = "b1";
    let body = multipart(boundary, "f.json", "\"appDataFolder\"", b"v1");
    let created: serde_json::Value = reqwest::Client::new()
        .post(&url)
        .header("Authorization", bearer(&token))
        .header(
            "Content-Type",
            format!("multipart/related; boundary={boundary}"),
        )
        .body(body)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let id = created["id"].as_str().unwrap().to_owned();

    let upd_url = format!("{}/upload/drive/v2/files/{id}?uploadType=media", base(&h));
    let resp = reqwest::Client::new()
        .put(&upd_url)
        .header("Authorization", bearer(&token))
        .header("If-Match", "\"v99\"")
        .body("x")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 412);
}

#[tokio::test]
async fn fake_drive_update_without_if_match_is_428() {
    let h = start().await;
    let (_, token) = setup(&h);
    let url = format!("{}/upload/drive/v3/files?uploadType=multipart", base(&h));
    let boundary = "b1";
    let body = multipart(boundary, "f.json", "\"appDataFolder\"", b"v1");
    let created: serde_json::Value = reqwest::Client::new()
        .post(&url)
        .header("Authorization", bearer(&token))
        .header(
            "Content-Type",
            format!("multipart/related; boundary={boundary}"),
        )
        .body(body)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let id = created["id"].as_str().unwrap().to_owned();

    let upd_url = format!("{}/upload/drive/v2/files/{id}?uploadType=media", base(&h));
    let resp = reqwest::Client::new()
        .put(&upd_url)
        .header("Authorization", bearer(&token))
        .body("x")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 428);
}

#[tokio::test]
async fn fake_drive_list_without_appdata_space_is_refused() {
    let h = start().await;
    let (_, token) = setup(&h);
    let url = format!("{}/drive/v3/files", base(&h));
    let resp = reqwest::Client::new()
        .get(&url)
        .header("Authorization", bearer(&token))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 403);
}

#[tokio::test]
async fn fake_drive_create_outside_appdata_is_403() {
    let h = start().await;
    let (_, token) = setup(&h);
    let url = format!("{}/upload/drive/v3/files?uploadType=multipart", base(&h));
    let boundary = "b1";
    let body = multipart(boundary, "f.json", "\"root\"", b"x");
    let resp = reqwest::Client::new()
        .post(&url)
        .header("Authorization", bearer(&token))
        .header(
            "Content-Type",
            format!("multipart/related; boundary={boundary}"),
        )
        .body(body)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 403);
}

#[tokio::test]
async fn fake_drive_gmail_only_token_is_403() {
    let h = start().await;
    let mb = h.add_mailbox("bob@example.com");
    let gmail_token = h.issue_token(&mb, &[GMAIL_MODIFY], time::Duration::hours(1));
    let url = format!("{}/drive/v3/files?spaces=appDataFolder", base(&h));
    let resp = reqwest::Client::new()
        .get(&url)
        .header("Authorization", bearer(&gmail_token))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 403);
}

#[tokio::test]
async fn fake_drive_user_deletes_file_then_list_is_empty() {
    let h = start().await;
    let (mb, token) = setup(&h);
    let url = format!("{}/upload/drive/v3/files?uploadType=multipart", base(&h));
    let boundary = "b1";
    let body = multipart(boundary, "f.json", "\"appDataFolder\"", b"x");
    let created: serde_json::Value = reqwest::Client::new()
        .post(&url)
        .header("Authorization", bearer(&token))
        .header(
            "Content-Type",
            format!("multipart/related; boundary={boundary}"),
        )
        .body(body)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let id = created["id"].as_str().unwrap().to_owned();
    assert_eq!(h.drive_files(&mb).len(), 1);

    h.user_deletes_app_files(&mb);
    assert_eq!(h.drive_files(&mb).len(), 0);

    let list_url = format!("{}/drive/v3/files?spaces=appDataFolder", base(&h));
    let list: serde_json::Value = reqwest::Client::new()
        .get(&list_url)
        .header("Authorization", bearer(&token))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(list["files"].as_array().unwrap().len(), 0);
    assert!(h.drive_file_bytes(&mb, &id).is_none());
}
