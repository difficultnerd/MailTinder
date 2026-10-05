//! Contract tests for `DriveAppFolder` against `fake-google` (T-405).
//!
//! The shared suite (`testkit::contract::app_folder_store`) runs here against
//! the Drive store and, in `testkit`, against the in-memory fake, so the two
//! cannot drift. The named tests below cover the story IDs T-405 owns.
#![allow(
    clippy::pedantic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::missing_panics_doc,
    clippy::missing_errors_doc
)]

mod support;

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use domain::MailboxId;
use fake_google::{FakeGoogleHandle, FakeMailboxKey, DRIVE_APPDATA};
use obs::Sensitive;
use ports::{AppFolderError, AppFolderStore, MailboxCtx};
use testkit::clock::VirtualClock;
use testkit::contract::app_folder_store::{app_folder_store, AppFolderTarget};
use testkit::T0;

/// A mailbox context carrying a fresh `drive.appdata` token.
fn ctx(handle: &FakeGoogleHandle, mb: &FakeMailboxKey, id: u128) -> MailboxCtx {
    let token = handle.issue_token(mb, &[DRIVE_APPDATA], time::Duration::hours(1));
    MailboxCtx {
        mailbox: MailboxId(uuid::Uuid::from_u128(id)),
        access_token: Sensitive::new(token),
    }
}

/// A Drive store on the fake's default clock.
fn store(handle: &FakeGoogleHandle) -> Arc<dyn AppFolderStore> {
    Arc::new(support::drive_store(handle, support::clock()))
}

/// The shared contract suite, run against `DriveAppFolder` over `fake-google`.
#[tokio::test]
async fn app_folder_store_contract_drive() {
    let handle = support::start().await;
    let counter = AtomicU32::new(1);
    let result = app_folder_store(|| async {
        let i = counter.fetch_add(1, Ordering::SeqCst);
        let mb = handle.add_mailbox(&format!("contract{i}@example.com"));
        AppFolderTarget {
            store: store(&handle),
            ctx: ctx(&handle, &mb, u128::from(i)),
            control: handle.app_folder_control(&mb),
        }
    })
    .await;
    assert_eq!(result, Ok(()), "drive contract failed: {result:?}");
}

/// SR-01 AC5: rules live in the user's app folder, not on our server.
#[tokio::test]
async fn sr_01_ac5_write_then_read_round_trip_ciphertext() {
    let handle = support::start().await;
    let mb = handle.add_mailbox("rules@example.com");
    let ctx = ctx(&handle, &mb, 7);
    let store = store(&handle);
    let ciphertext = b"CIPHERTEXT-round-trip-1".to_vec();
    let tag = store.write(&ctx, &ciphertext, None).await.expect("write");
    let read = store.read(&ctx).await.expect("read").expect("present");
    assert_eq!(read.0, ciphertext);
    assert_eq!(read.1, tag);
}

/// AU-05 AC4 (store half): the file can be read from one mailbox's Drive,
/// written to another's and deleted from the first.
#[tokio::test]
async fn au_05_ac4_copy_between_two_drives_then_delete_old() {
    let handle = support::start().await;
    let primary = handle.add_mailbox("primary@example.com");
    let next = handle.add_mailbox("next@example.com");
    let ctx_primary = ctx(&handle, &primary, 21);
    let ctx_next = ctx(&handle, &next, 22);
    let store_primary = store(&handle);
    let store_next = store(&handle);
    let state = b"state-to-move".to_vec();
    store_primary
        .write(&ctx_primary, &state, None)
        .await
        .expect("write primary");
    let moved = store_primary
        .read(&ctx_primary)
        .await
        .expect("read primary")
        .expect("present");
    store_next
        .write(&ctx_next, &moved.0, None)
        .await
        .expect("write next");
    let at_next = store_next
        .read(&ctx_next)
        .await
        .expect("read next")
        .expect("present");
    assert_eq!(at_next.0, state);
    store_primary
        .delete(&ctx_primary)
        .await
        .expect("delete primary");
    assert!(store_primary
        .read(&ctx_primary)
        .await
        .expect("read primary after")
        .is_none());
    assert!(store_next
        .read(&ctx_next)
        .await
        .expect("read next after")
        .is_some());
}

/// AU-06 AC1 (store half): deleting the app folder file is idempotent.
#[tokio::test]
async fn au_06_ac1_delete_is_idempotent() {
    let handle = support::start().await;
    let mb = handle.add_mailbox("delete@example.com");
    let ctx = ctx(&handle, &mb, 31);
    let store = store(&handle);
    store.write(&ctx, b"a", None).await.expect("write");
    store.delete(&ctx).await.expect("delete 1");
    store.delete(&ctx).await.expect("delete 2");
    assert!(store.read(&ctx).await.expect("read").is_none());
    assert!(handle.drive_files(&mb).is_empty());
}

/// A stale `If-Match` is a conflict and leaves the bytes untouched.
#[tokio::test]
async fn app_folder_stale_etag_is_conflict() {
    let handle = support::start().await;
    let mb = handle.add_mailbox("stale@example.com");
    let ctx = ctx(&handle, &mb, 11);
    let store = store(&handle);
    let tag = store.write(&ctx, b"one", None).await.expect("write");
    let tag2 = store.write(&ctx, b"two", Some(&tag)).await.expect("write");
    match store.write(&ctx, b"three", Some(&tag)).await {
        Err(AppFolderError::Conflict) => {}
        other => panic!("expected conflict, got {other:?}"),
    }
    let read = store.read(&ctx).await.expect("read").expect("present");
    assert_eq!(read.0.as_slice(), b"two");
    assert_eq!(read.1, tag2);
}

/// `write(None)` when a file already exists is a conflict, never an overwrite.
#[tokio::test]
async fn app_folder_create_when_exists_is_conflict() {
    let handle = support::start().await;
    let mb = handle.add_mailbox("exists@example.com");
    let ctx = ctx(&handle, &mb, 13);
    let store = store(&handle);
    store.write(&ctx, b"a", None).await.expect("first");
    match store.write(&ctx, b"b", None).await {
        Err(AppFolderError::Conflict) => {}
        other => panic!("expected conflict, got {other:?}"),
    }
    let read = store.read(&ctx).await.expect("read").expect("present");
    assert_eq!(read.0.as_slice(), b"a");
}

/// The user deleting the file reads as absent and invalidates the old ETag.
#[tokio::test]
async fn app_folder_user_deleted_file_reads_none() {
    let handle = support::start().await;
    let mb = handle.add_mailbox("deleted@example.com");
    let ctx = ctx(&handle, &mb, 15);
    let store = store(&handle);
    let tag = store.write(&ctx, b"a", None).await.expect("write");
    handle.user_deletes_app_files(&mb);
    assert!(store.read(&ctx).await.expect("read").is_none());
    match store.write(&ctx, b"b", Some(&tag)).await {
        Err(AppFolderError::Conflict) => {}
        other => panic!("expected conflict, got {other:?}"),
    }
}

/// Two raced creates leave two files; the newest `modifiedTime` is used.
#[tokio::test]
async fn app_folder_duplicates_pick_latest() {
    let clock = Arc::new(VirtualClock::new(T0));
    let handle = support::start_on_clock(Arc::clone(&clock)).await;
    let mb = handle.add_mailbox("duplicates@example.com");
    let token = handle.issue_token(&mb, &[DRIVE_APPDATA], time::Duration::hours(1));
    let store = support::drive_store(&handle, clock.clone());
    let ctx = MailboxCtx {
        mailbox: MailboxId(uuid::Uuid::from_u128(42)),
        access_token: Sensitive::new(token.clone()),
    };
    raw_create(&handle, &token, b"older").await;
    clock.advance(time::Duration::seconds(60));
    raw_create(&handle, &token, b"newer").await;
    let read = store.read(&ctx).await.expect("read").expect("present");
    assert_eq!(read.0.as_slice(), b"newer");
}

/// Every list asks for `appDataFolder` and create writes only there: a
/// successful create is one the fake accepts only with `parents=["appDataFolder"]`.
#[tokio::test]
async fn app_folder_uses_only_drive_appdata_space() {
    let handle = support::start().await;
    let mb = handle.add_mailbox("space@example.com");
    let ctx = ctx(&handle, &mb, 9);
    let store = store(&handle);
    store.write(&ctx, b"x", None).await.expect("write");
    store.read(&ctx).await.expect("read");
    let events = handle.events();
    let lists: Vec<Vec<(String, String)>> = events
        .iter()
        .filter_map(|e| match e {
            fake_google::FakeEvent::Request { route, query, .. } if route == "drive.files.list" => {
                Some(query.clone())
            }
            _ => None,
        })
        .collect();
    assert!(!lists.is_empty(), "expected at least one list call");
    for query in lists {
        assert!(
            query
                .iter()
                .any(|(k, v)| k == "spaces" && v == "appDataFolder"),
            "list must request spaces=appDataFolder, saw {query:?}"
        );
    }
    let creates = events
        .iter()
        .filter(|e| {
            matches!(
                e,
                fake_google::FakeEvent::Request { route, .. } if route == "drive.files.create"
            )
        })
        .count();
    assert_eq!(creates, 1, "expected exactly one create");
}

/// Create one app-data file directly over HTTP, bypassing the store, so a test
/// can set up a duplicate the store itself would refuse.
async fn raw_create(handle: &FakeGoogleHandle, token: &str, data: &[u8]) -> String {
    let boundary = "dup-boundary";
    let meta = format!(
        "{{\"name\":\"{}\",\"parents\":[\"appDataFolder\"]}}",
        app_file_name()
    );
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
    let url = format!(
        "{}upload/drive/v3/files?uploadType=multipart",
        handle.base_url()
    );
    let resp = reqwest::Client::new()
        .post(&url)
        .header("Authorization", format!("Bearer {token}"))
        .header(
            "Content-Type",
            format!("multipart/related; boundary={boundary}"),
        )
        .body(body)
        .send()
        .await
        .expect("create");
    assert_eq!(resp.status(), 200, "raw create status");
    let created: serde_json::Value = resp.json().await.expect("json");
    created["id"].as_str().expect("id").to_owned()
}

/// The app folder file name the store uses (kept in one place).
fn app_file_name() -> &'static str {
    "mailtinder-state-v1.bin"
}
