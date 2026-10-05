//! Contract tests for the Gmail modify half against `fake-google` (T-403).
//!
//! Every change goes through the `MailProvider` trait; the fake records the
//! requests and exposes the mailbox so the tests can assert the exact labels
//! before and after each call, and that no permanent delete ever happens.
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
use fake_google::{FakeEvent, FakeGoogleHandle, FakeMailboxKey};
use ports::MailProvider;
use testkit::contract::mail_provider::{mail_provider, CaseGroups};
use time::OffsetDateTime;

/// The default internal date for a seeded message.
fn t0() -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(500).unwrap()
}

/// A minimal `.eml` with the given subject.
fn eml(subject: &str) -> Vec<u8> {
    format!("From: a@example.com\r\nSubject: {subject}\r\n\r\nbody").into_bytes()
}

/// Seed one message with `labels` and return its ID.
fn seed(handle: &FakeGoogleHandle, mb: &FakeMailboxKey, labels: &[&str]) -> MessageId {
    let id = handle.seed_eml(mb, &eml("x"), labels, t0());
    MessageId::new(id).expect("message id")
}

/// The label set the mailbox holds for the message.
fn labels_of(handle: &FakeGoogleHandle, mb: &FakeMailboxKey, id: &MessageId) -> LabelSet {
    let stored = handle.labels_of(mb, id.as_str()).expect("stored labels");
    LabelSet::from_ids(stored)
}

/// The number of `POST /labels` requests the fake has seen.
fn label_create_count(handle: &FakeGoogleHandle) -> usize {
    handle
        .events()
        .into_iter()
        .filter(|e| {
            matches!(
                e,
                FakeEvent::Request { method, route, .. }
                    if route == "labels.create" && method == "POST"
            )
        })
        .count()
}

/// The routes the fake has seen, for asserting which calls were made.
fn routes(handle: &FakeGoogleHandle) -> Vec<String> {
    handle
        .events()
        .into_iter()
        .filter_map(|e| match e {
            FakeEvent::Request { route, .. } => Some(route),
            _ => None,
        })
        .collect()
}

/// The whole modify case set of the shared `MailProvider` contract suite.
#[tokio::test]
async fn mail_provider_contract_modify_gmail() -> Result<(), String> {
    let handle = support::start().await;
    let counter = AtomicU32::new(0);
    let make = || {
        let email = format!(
            "modify{}@example.com",
            counter.fetch_add(1, Ordering::SeqCst)
        );
        let target = support::target(&handle, &email);
        async move { target }
    };
    let outcome = mail_provider(
        make,
        CaseGroups {
            read: true,
            modify: true,
            send: false,
        },
    )
    .await;
    outcome.as_ref()?;
    if handle.permanent_delete_attempts() != 0 {
        return Err("the suite attempted a permanent delete".into());
    }
    Ok(())
}

/// INV-5: after the whole modify suite, `fake-google` recorded no permanent
/// delete attempt, because no adapter path ever issues one.
#[tokio::test]
async fn inv_5_fake_google_records_no_permanent_delete() -> Result<(), String> {
    let handle = support::start().await;
    let counter = AtomicU32::new(0);
    let make = || {
        let email = format!("inv5{}@example.com", counter.fetch_add(1, Ordering::SeqCst));
        let target = support::target(&handle, &email);
        async move { target }
    };
    mail_provider(
        make,
        CaseGroups {
            read: true,
            modify: true,
            send: false,
        },
    )
    .await?;
    assert_eq!(handle.permanent_delete_attempts(), 0);
    Ok(())
}

/// SW-03 AC1: reject trashes the message (never a delete) and hands back the
/// labels as they were before, which is the undo state.
#[tokio::test]
async fn sw_03_ac1_trash_returns_labels_before() {
    let handle = support::start().await;
    let mb = handle.add_mailbox("trash@example.com");
    let provider = support::provider(&handle, support::clock());
    let ctx = support::mail_ctx(&handle, &mb);
    let id = seed(&handle, &mb, &["INBOX", "UNREAD"]);

    let before = provider.trash(&ctx, &id).await.unwrap();

    assert_eq!(
        before,
        LabelSet::from_ids(["INBOX", "UNREAD"].map(str::to_owned))
    );
    let after = labels_of(&handle, &mb, &id);
    assert!(after.contains("TRASH"), "no TRASH label after a trash");
    assert!(!after.contains("INBOX"));
    assert_eq!(handle.permanent_delete_attempts(), 0);
}

/// SW-03 AC3: a suspected-spam swipe adds `SPAM` and removes `INBOX`, and
/// returns the labels before the report so undo can put them back.
#[tokio::test]
async fn sw_03_ac3_report_spam_adds_spam_removes_inbox() {
    let handle = support::start().await;
    let mb = handle.add_mailbox("spam@example.com");
    let provider = support::provider(&handle, support::clock());
    let ctx = support::mail_ctx(&handle, &mb);
    let id = seed(&handle, &mb, &["INBOX", "UNREAD"]);

    let before = provider.report_spam(&ctx, &id).await.unwrap();

    assert_eq!(
        before,
        LabelSet::from_ids(["INBOX", "UNREAD"].map(str::to_owned))
    );
    let after = labels_of(&handle, &mb, &id);
    assert!(after.contains("SPAM"), "the report must add SPAM");
    assert!(!after.contains("INBOX"));
}

/// SW-04 AC2 / FL-02 AC1: filing creates the label, applies it and takes the
/// message out of the inbox.
#[tokio::test]
async fn sw_04_ac2_file_adds_label_removes_inbox() {
    let handle = support::start().await;
    let mb = handle.add_mailbox("file@example.com");
    let provider = support::provider(&handle, support::clock());
    let ctx = support::mail_ctx(&handle, &mb);
    let id = seed(&handle, &mb, &["INBOX", "UNREAD"]);

    let label = provider.ensure_label(&ctx, "Receipts").await.unwrap();
    let add = LabelSet::from_ids([label.clone()]);
    let remove = LabelSet::from_ids(["INBOX".to_owned()]);
    let after = provider.set_labels(&ctx, &id, &add, &remove).await.unwrap();

    let expected = LabelSet::from_ids(["UNREAD".to_owned(), label]);
    assert_eq!(after, expected);
    assert_eq!(labels_of(&handle, &mb, &id), expected);
}

/// SW-05 AC1: undo after a trash restores the exact previous labels, byte for
/// byte, including a user label and a category.
#[tokio::test]
async fn sw_05_ac1_restore_after_trash_is_exact() {
    let handle = support::start().await;
    let mb = handle.add_mailbox("undotrash@example.com");
    let provider = support::provider(&handle, support::clock());
    let ctx = support::mail_ctx(&handle, &mb);
    let id = seed(
        &handle,
        &mb,
        &["INBOX", "UNREAD", "CATEGORY_PROMOTIONS", "Label_7"],
    );

    let before = provider.trash(&ctx, &id).await.unwrap();
    assert_eq!(
        before,
        LabelSet::from_ids(
            ["INBOX", "UNREAD", "CATEGORY_PROMOTIONS", "Label_7"].map(str::to_owned)
        )
    );

    provider.restore_labels(&ctx, &id, &before).await.unwrap();
    assert_eq!(labels_of(&handle, &mb, &id), before);
}

/// SW-05 AC1: undo after a file puts the message back in the inbox and
/// removes the filing label.
#[tokio::test]
async fn sw_05_ac1_restore_after_file_is_exact() {
    let handle = support::start().await;
    let mb = handle.add_mailbox("undofile@example.com");
    let provider = support::provider(&handle, support::clock());
    let ctx = support::mail_ctx(&handle, &mb);
    let id = seed(&handle, &mb, &["INBOX", "UNREAD"]);
    let before = labels_of(&handle, &mb, &id);

    let label = provider.ensure_label(&ctx, "Receipts").await.unwrap();
    provider
        .set_labels(
            &ctx,
            &id,
            &LabelSet::from_ids([label]),
            &LabelSet::from_ids(["INBOX".to_owned()]),
        )
        .await
        .unwrap();

    provider.restore_labels(&ctx, &id, &before).await.unwrap();
    assert_eq!(labels_of(&handle, &mb, &id), before);
}

/// SW-05 AC4a: a spam report is undone by restoring the labels; adding `SPAM`
/// is reversed by removing it.
#[tokio::test]
async fn sw_05_ac4a_restore_after_spam_is_exact() {
    let handle = support::start().await;
    let mb = handle.add_mailbox("undospam@example.com");
    let provider = support::provider(&handle, support::clock());
    let ctx = support::mail_ctx(&handle, &mb);
    let id = seed(&handle, &mb, &["INBOX", "UNREAD"]);
    let before = labels_of(&handle, &mb, &id);

    provider.report_spam(&ctx, &id).await.unwrap();
    assert!(labels_of(&handle, &mb, &id).contains("SPAM"));

    provider.restore_labels(&ctx, &id, &before).await.unwrap();
    assert_eq!(labels_of(&handle, &mb, &id), before);
}

/// FL-02 AC1: `ensure_label` creates the label once, reuses it (case
/// insensitively) and survives the `409` race the fake injects.
#[tokio::test]
async fn fl_02_ac1_ensure_label_creates_once_then_reuses() {
    let handle = support::start().await;
    let provider = support::provider(&handle, support::clock());

    // A fresh mailbox: the first call creates, the second reuses.
    let mb = handle.add_mailbox("labels@example.com");
    let ctx = support::mail_ctx(&handle, &mb);
    let first = provider.ensure_label(&ctx, "Receipts").await.unwrap();
    let second = provider.ensure_label(&ctx, "receipts").await.unwrap();
    assert_eq!(first, second, "a case-different name must reuse the label");
    assert_eq!(
        label_create_count(&handle),
        1,
        "the label was created twice"
    );

    // A raced mailbox: the fake makes the label and answers 409; the adapter
    // must list again and return the ID rather than fail.
    let racy = handle.add_mailbox("race@example.com");
    handle.arm_label_create_race(&racy);
    let ctx = support::mail_ctx(&handle, &racy);
    let raced = provider.ensure_label(&ctx, "Race").await.unwrap();
    assert!(!raced.is_empty());
    assert_eq!(
        label_create_count(&handle),
        2,
        "the raced create must run exactly once"
    );
    let again = provider.ensure_label(&ctx, "race").await.unwrap();
    assert_eq!(again, raced, "the raced label must now be reused");
    assert_eq!(label_create_count(&handle), 2);
}

/// S8: a user label the mailbox no longer has is skipped, not sent to
/// `modify`, and the restore still succeeds.
#[tokio::test]
async fn gmail_restore_skips_deleted_user_label() {
    let handle = support::start().await;
    let mb = handle.add_mailbox("gone-label@example.com");
    let provider = support::provider(&handle, support::clock());
    let ctx = support::mail_ctx(&handle, &mb);
    let id = seed(&handle, &mb, &["INBOX"]);

    // `Label_999` was on the message in an earlier session; it is no longer a
    // label of this mailbox, so it must be dropped from the add set.
    let exact = LabelSet::from_ids(["INBOX".to_owned(), "Label_999".to_owned()]);
    provider.restore_labels(&ctx, &id, &exact).await.unwrap();

    assert_eq!(
        labels_of(&handle, &mb, &id),
        LabelSet::from_ids(["INBOX".to_owned()])
    );
    assert!(
        routes(&handle).iter().any(|r| r == "labels.list"),
        "the skip must consult labels.list"
    );
    assert!(
        !routes(&handle).iter().any(|r| r == "labels.create"),
        "the skip must not create a missing label"
    );
}

/// INV-6: for any label set drawn from the mailbox's label universe, a change
/// followed by a restore returns the original set exactly.
#[tokio::test]
async fn inv_6_restore_property_any_label_set() {
    let handle = support::start().await;
    let mb = handle.add_mailbox("property@example.com");
    let provider = support::provider(&handle, support::clock());
    let ctx = support::mail_ctx(&handle, &mb);
    let user = provider.ensure_label(&ctx, "Project").await.unwrap();
    let universe = [
        "INBOX".to_owned(),
        "UNREAD".to_owned(),
        "STARRED".to_owned(),
        "IMPORTANT".to_owned(),
        "CATEGORY_PERSONAL".to_owned(),
        user,
    ];

    let mut rng = XorShift::new(0x5eed_1234_abcd_0001);
    for round in 0..16u32 {
        let s = rng.subset(universe.len());
        let r = rng.subset(universe.len());
        let a = rng.subset(universe.len()) & !r;
        let start = LabelSet::from_ids(pick(&universe, s));
        let add = LabelSet::from_ids(pick(&universe, a));
        let remove = LabelSet::from_ids(pick(&universe, r));
        let id = seed(
            &handle,
            &mb,
            &start.iter().map(String::as_str).collect::<Vec<_>>(),
        );

        let after = provider.set_labels(&ctx, &id, &add, &remove).await.unwrap();
        let mut expected = start.clone();
        for label in add.iter() {
            expected.insert(label.clone());
        }
        for label in remove.iter() {
            expected.remove(label);
        }
        assert_eq!(after, expected, "round {round}: change was not as computed");

        provider.restore_labels(&ctx, &id, &start).await.unwrap();
        assert_eq!(
            labels_of(&handle, &mb, &id),
            start,
            "round {round}: restore did not return the original set"
        );
    }
}

/// The indices of `universe` whose bit is set in `mask`.
fn pick(universe: &[String], mask: u64) -> Vec<String> {
    universe
        .iter()
        .enumerate()
        .filter(|(i, _)| mask & (1 << i) != 0)
        .map(|(_, s)| s.clone())
        .collect()
}

/// A mask over `n` items with at least one bit the caller may clear.
fn rng_subset_mask(n: usize) -> u64 {
    (1u64 << n) - 1
}

/// A tiny deterministic generator so the property test needs no `rand` crate.
struct XorShift(u64);

impl XorShift {
    fn new(seed: u64) -> Self {
        Self(seed)
    }

    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    /// A bitmask over `n` items.
    fn subset(&mut self, n: usize) -> u64 {
        self.next() & rng_subset_mask(n)
    }
}
