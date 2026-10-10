//! The T-202a `ServerStore` contract suite against the Firestore emulator,
//! plus the emulator-specific tests T-301 requires.
//!
//! Only compiled when the `firestore-emulator` feature is on (CI runs
//! `cargo test --all-features`). The emulator must be reachable at
//! `FIRESTORE_EMULATOR_HOST` (default `127.0.0.1:8085`).
#![cfg(feature = "firestore-emulator")]
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::net::SocketAddr;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use adapters_gcp::{FirestoreConfig, FirestoreStore, GcpHttp, StaticTokenSource, TokenSource};
use ports::store::{Precondition, RateLimitKey, ServerStore, StoreError};
use testkit::contract::server_store::{samples, server_store};
use time::{Duration, OffsetDateTime};

/// A fresh project ID per call so each contract case starts empty without
/// deleting anything.
fn project_id() -> String {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("test-{n}-{}", std::process::id())
}

/// Build a `FirestoreStore` against the emulator with a fresh project.
fn make_store() -> Arc<dyn ServerStore> {
    let host =
        std::env::var("FIRESTORE_EMULATOR_HOST").unwrap_or_else(|_| "127.0.0.1:8085".to_owned());
    let addr: SocketAddr = host.parse().expect("valid emulator host");
    let tokens: Arc<dyn TokenSource> =
        Arc::new(StaticTokenSource(obs::Sensitive::new("owner".to_owned())));
    let http = Arc::new(GcpHttp::with_emulator(tokens, addr).expect("emulator client"));
    let cfg = FirestoreConfig::new(project_id());
    Arc::new(FirestoreStore::new(http, cfg))
}

#[tokio::test]
async fn server_store_contract_firestore_emulator() -> Result<(), String> {
    server_store(|| async { make_store() }).await
}

/// RL-1: 20 concurrent `hit`s on the same window return 1..=20 with no
/// duplicates, proving the counter increments atomically.
#[tokio::test]
async fn rl_1_rate_limit_hit_increments_atomically() -> Result<(), String> {
    let store = make_store();
    let key = RateLimitKey("rl-atomic".into());
    let ws = OffsetDateTime::now_utc();
    let window = Duration::minutes(1);

    let mut handles = Vec::new();
    for _ in 0..20 {
        let store = Arc::clone(&store);
        let key = key.clone();
        handles.push(tokio::spawn(async move {
            store.rate_limits().hit(&key, ws, window).await
        }));
    }
    let mut counts = Vec::new();
    for h in handles {
        let joined = h.await.map_err(|e| format!("join: {e}"))?;
        counts.push(joined.map_err(|e| format!("hit: {e:?}"))?);
    }
    counts.sort_unstable();
    let expected: Vec<u32> = (1..=20).collect();
    if counts != expected {
        return Err("counts must be 1..=20 with no duplicates".into());
    }
    Ok(())
}

/// A conditional `put` with a stale `Matches` version maps to
/// `PreconditionFailed` (the undo race depends on this).
#[tokio::test]
async fn firestore_conditional_put_conflict_is_precondition_failed() -> Result<(), String> {
    let store = make_store();
    let u = samples::user(1);
    let v1 = store
        .users()
        .put(&u, Precondition::None)
        .await
        .map_err(|e| format!("first put: {e:?}"))?;
    // A stale version must fail with PreconditionFailed.
    let stale = Precondition::Matches(ports::store::Version("2020-01-01T00:00:00.000000Z".into()));
    let err = store
        .users()
        .put(&u, stale)
        .await
        .err()
        .ok_or_else(|| "stale put should fail".to_owned())?;
    if !matches!(err, StoreError::PreconditionFailed) {
        return Err(format!("expected PreconditionFailed, got {err:?}"));
    }
    // The current version still succeeds.
    store
        .users()
        .put(&u, Precondition::Matches(v1))
        .await
        .map_err(|e| format!("current put: {e:?}"))?;
    Ok(())
}

/// T-1101a: a missing `config/classifiers` document reads as `Ok(None)`, not an
/// error. The emulator answers the GET with `404 NOT_FOUND`; the adapter must
/// map that to "no configuration" (classifiers off) so the Feed never fails on
/// an unconfigured deployment.
#[tokio::test]
async fn config_classifiers_missing_document_is_none_not_error() -> Result<(), String> {
    let store = make_store();
    let got = store
        .config()
        .get_classifiers()
        .await
        .map_err(|e| format!("a missing config document must not error, got {e:?}"))?;
    if got.is_some() {
        return Err("a fresh project has no config document".into());
    }
    Ok(())
}
