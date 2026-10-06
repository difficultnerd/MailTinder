//! Contract tests for the typed query, count and label calls against
//! `fake-google` (T-602a).
#![allow(
    clippy::pedantic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::missing_panics_doc,
    clippy::missing_errors_doc
)]

mod support;

use fake_google::FakeEvent;
use ports::{MailProvider, MessageQuery};
use testkit::contract::mail_provider as contract;

#[tokio::test]
async fn xc_02_list_messages_contract_inbox_and_dates() {
    let handle = support::start().await;
    let target = support::target(&handle, "q1@example.com");
    assert_eq!(
        contract::list_messages_inbox_and_dates(&target).await,
        Ok(())
    );
}

#[tokio::test]
async fn xc_02_list_messages_contract_label_and_sender() {
    let handle = support::start().await;
    let target = support::target(&handle, "q2@example.com");
    assert_eq!(
        contract::list_messages_label_and_sender(&target).await,
        Ok(())
    );
}

#[tokio::test]
async fn xc_02_count_messages_contract_label_exact() {
    let handle = support::start().await;
    let target = support::target(&handle, "q3@example.com");
    assert_eq!(contract::count_messages_label_exact(&target).await, Ok(()));
}

#[tokio::test]
async fn xc_02_rename_label_contract() {
    let handle = support::start().await;
    let target = support::target(&handle, "q4@example.com");
    assert_eq!(contract::rename_label_cases(&target).await, Ok(()));
}

#[tokio::test]
async fn inv_5_remove_label_keeps_messages() {
    let handle = support::start().await;
    let target = support::target(&handle, "q5@example.com");
    assert_eq!(contract::remove_label_keeps_messages(&target).await, Ok(()));
}

#[tokio::test]
async fn inv_5_no_permanent_delete_during_label_removal() {
    let handle = support::start().await;
    let target = support::target(&handle, "q6@example.com");
    assert_eq!(contract::remove_label_keeps_messages(&target).await, Ok(()));
    let events = handle.events();
    assert!(events.iter().any(|e| matches!(
        e,
        FakeEvent::Request { route, .. } if route == "labels.delete"
    )));
    // `permanent_delete_attempts` is the fake's own counter; it stays zero
    // because removing a label never removes a message (INV-5).
    assert_eq!(handle.permanent_delete_attempts(), 0);
}

#[tokio::test]
async fn gm_04_ac1_year_count_is_one_call() {
    let handle = support::start().await;
    let target = support::target(&handle, "q7@example.com");
    // Seed through the contract helper so the count has something to find.
    assert_eq!(
        contract::list_messages_inbox_and_dates(&target).await,
        Ok(())
    );
    let before = list_requests(&handle);
    let gets_before = get_requests(&handle);
    let q = MessageQuery {
        after: Some(time::OffsetDateTime::from_unix_timestamp(100).unwrap()),
        before: Some(time::OffsetDateTime::from_unix_timestamp(400).unwrap()),
        ..MessageQuery::default()
    };
    let n = target
        .provider
        .count_messages(&target.ctx, &q)
        .await
        .unwrap();
    assert_eq!(n, 3);
    assert_eq!(list_requests(&handle) - before, 1);
    let gets_after = get_requests(&handle);
    assert_eq!(
        gets_after - gets_before,
        0,
        "the count made no per-message call"
    );
}

fn list_requests(handle: &fake_google::FakeGoogleHandle) -> usize {
    handle
        .events()
        .iter()
        .filter(|e| matches!(e, FakeEvent::Request { route, .. } if route == "messages.list"))
        .count()
}

#[tokio::test]
async fn xc_02_invalid_query_makes_no_provider_call() {
    let handle = support::start().await;
    let mb = handle.add_mailbox("q8@example.com");
    let provider = support::refusing_provider(&handle);
    let ctx = support::mail_ctx(&handle, &mb);
    let q = MessageQuery {
        from: Some("a\"b@example.com".to_owned()),
        ..MessageQuery::default()
    };
    // A refusing egress would answer `egress_refused` if a request were made.
    assert_eq!(
        provider.count_messages(&ctx, &q).await,
        Err(ports::MailError::Invalid("query".to_owned()))
    );
    assert_eq!(
        provider.list_messages(&ctx, &q, None, 10).await.map(|_| ()),
        Err(ports::MailError::Invalid("query".to_owned()))
    );
}

fn get_requests(handle: &fake_google::FakeGoogleHandle) -> usize {
    handle
        .events()
        .iter()
        .filter(|e| matches!(e, FakeEvent::Request { route, .. } if route == "messages.get"))
        .count()
}
