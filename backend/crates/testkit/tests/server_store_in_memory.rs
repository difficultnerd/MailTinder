//! Tests for the in-memory `ServerStore` (T-202a).

use std::sync::Arc;

use domain::UserId;
use ports::store::{Precondition, ServerStore};
use proptest::prelude::*;
use testkit::contract::server_store::server_store;
use testkit::store::InMemoryServerStore;
use uuid::Uuid;

#[tokio::test]
async fn server_store_contract_in_memory() {
    let result =
        server_store(|| async { Arc::new(InMemoryServerStore::new()) as Arc<dyn ServerStore> })
            .await;
    assert_eq!(result, Ok(()), "contract failed: {result:?}");
}

#[tokio::test]
async fn in_memory_fail_next_returns_unavailable() {
    let store = InMemoryServerStore::new();
    store.fail_next(1);
    let u = testkit::contract::server_store::samples::user(1);
    let err = store.users().put(&u, Precondition::None).await;
    assert!(matches!(err, Err(ports::store::StoreError::Unavailable)));
    // The next call succeeds.
    let ok = store.users().put(&u, Precondition::None).await;
    assert!(ok.is_ok());
}

#[tokio::test]
async fn in_memory_export_json_lists_every_collection_written() {
    let store = InMemoryServerStore::new();
    let u = testkit::contract::server_store::samples::user(1);
    let mb = testkit::contract::server_store::samples::mailbox(&u.user_id, 1);
    let inv = testkit::contract::server_store::samples::invite(1);
    let ir = testkit::contract::server_store::samples::invite_request(1);
    let job = testkit::contract::server_store::samples::job(&u.user_id, &mb.mailbox_id, 1);
    let na =
        testkit::contract::server_store::samples::needs_attention(&u.user_id, &mb.mailbox_id, 1);
    let sess = testkit::contract::server_store::samples::session(Some(&u.user_id), 1);
    let eval = testkit::contract::server_store::samples::eval("pseudo", 1);
    let snap = testkit::contract::server_store::samples::snapshot(1);
    store
        .users()
        .put(&u, Precondition::None)
        .await
        .unwrap_or_else(|_| panic!("put"));
    store
        .mailboxes()
        .put(&mb, Precondition::None)
        .await
        .unwrap_or_else(|_| panic!("put"));
    store
        .invites()
        .put(&inv, Precondition::None)
        .await
        .unwrap_or_else(|_| panic!("put"));
    store
        .invite_requests()
        .put(&ir, Precondition::None)
        .await
        .unwrap_or_else(|_| panic!("put"));
    store
        .jobs()
        .put(&job, Precondition::None)
        .await
        .unwrap_or_else(|_| panic!("put"));
    store
        .needs_attention()
        .put(&na, Precondition::None)
        .await
        .unwrap_or_else(|_| panic!("put"));
    store
        .sessions()
        .put(&sess, Precondition::None)
        .await
        .unwrap_or_else(|_| panic!("put"));
    store
        .classifier_eval()
        .put(&eval, Precondition::None)
        .await
        .unwrap_or_else(|_| panic!("put"));
    store
        .bakeoff_snapshots()
        .put(&snap, Precondition::None)
        .await
        .unwrap_or_else(|_| panic!("put"));

    let out = store.export_json();
    let collections: Vec<&str> = out.iter().map(|(c, _)| *c).collect();
    for c in [
        "users",
        "mailboxes",
        "invites",
        "invite_requests",
        "jobs",
        "needs_attention",
        "sessions",
        "classifier_eval",
        "bakeoff_snapshots",
    ] {
        assert!(collections.contains(&c), "missing collection {c}");
    }
}

proptest! {
    #[test]
    fn in_memory_versions_never_repeat(ops in proptest::collection::vec(0u8..3, 0..50)) {
        let rt = tokio::runtime::Runtime::new().unwrap_or_else(|_| panic!("rt"));
        let store = InMemoryServerStore::new();
        let mut seen = std::collections::HashSet::new();
        for op in ops {
            let u = UserId(Uuid::from_u128(u128::from(op)));
            match op % 3 {
                0 => {
                    let rec = testkit::contract::server_store::samples::user(op);
                    if let Ok(v) = rt.block_on(store.users().put(&rec, Precondition::None)) {
                        assert!(seen.insert(v.0.clone()), "version repeated: {}", v.0);
                    }
                }
                1 => {
                    let _ = rt.block_on(store.users().delete(&u, Precondition::None));
                }
                _ => {
                    let _ = rt.block_on(store.users().get(&u));
                }
            }
        }
    }
}
