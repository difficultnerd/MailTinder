//! T-305: the `ClassifierSwitches` kill switches against the in-memory
//! `ServerStore` with a `VirtualClock`, so refresh is driven by advancing the
//! clock instead of sleeping (S4 5.7 "checked every minute"; CL-04 AC1,
//! CFG-1; JEV-1).

#![cfg(feature = "test-support")]
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::sync::Arc;

use adapters_gcp::{BakeoffModel, ClassifierSwitches, EnvSwitches};
use ports::store::{
    BakeoffSnapshotRepo, ClassifierEvalRepo, ClassifiersConfig, ConfigRepo, InviteRepo,
    InviteRequestRepo, JobRepo, MailboxRepo, NeedsAttentionRepo, Precondition, RateLimitRepo,
    ServerStore, SessionRepo, StoreError, UserRepo, Version, Versioned,
};
use ports::Clock;
use testkit::{InMemoryServerStore, VirtualClock, T0};
use time::Duration;

fn switches(
    env: EnvSwitches,
    store: Arc<dyn ServerStore>,
    clock: Arc<dyn Clock>,
) -> ClassifierSwitches {
    ClassifierSwitches::new(env, store, clock)
}

fn cfg(gemini: bool, jev: bool) -> ClassifiersConfig {
    ClassifiersConfig {
        gemini_enabled: gemini,
        jev_enabled: jev,
        updated_at: T0,
    }
}

async fn seed(store: &InMemoryServerStore, doc: ClassifiersConfig) -> Result<(), String> {
    store
        .config()
        .put_classifiers(&doc, Precondition::MustNotExist)
        .await
        .map_err(|e| format!("{e:?}"))?;
    Ok(())
}

/// An admin switch-off on instance A reaches instance B within one refresh
/// (`CONFIG_REFRESH`), without a deploy, and Gemini is unaffected.
#[tokio::test]
async fn cl_04_ac1_switch_off_seen_within_one_refresh() -> Result<(), String> {
    let store = Arc::new(InMemoryServerStore::new());
    seed(&store, cfg(true, true)).await?;
    let clock = Arc::new(VirtualClock::new(T0));
    let env = EnvSwitches {
        gemini: true,
        jev: true,
    };
    let a = switches(
        env,
        Arc::clone(&store) as Arc<dyn ServerStore>,
        Arc::clone(&clock) as Arc<dyn Clock>,
    );
    let b = switches(
        env,
        Arc::clone(&store) as Arc<dyn ServerStore>,
        Arc::clone(&clock) as Arc<dyn Clock>,
    );
    // Both instances read the document at T0 while Jev is on.
    if !a.enabled(BakeoffModel::Jev).await {
        return Err("A should see Jev on initially".into());
    }
    if !b.enabled(BakeoffModel::Jev).await {
        return Err("B should see Jev on initially".into());
    }
    // Admin A turns Jev off.
    let written = a
        .set(BakeoffModel::Jev, false)
        .await
        .map_err(|e| format!("{e:?}"))?;
    if written.jev || !written.gemini {
        return Err("set should produce Jev off, Gemini on".into());
    }
    // B has not refreshed yet: still on.
    if !b.enabled(BakeoffModel::Jev).await {
        return Err("B should still see Jev on before the refresh window".into());
    }
    // Gemini is unaffected on B.
    if !b.enabled(BakeoffModel::Gemini).await {
        return Err("Gemini should stay on".into());
    }
    // Advance past one refresh window.
    clock.advance(Duration::seconds(61));
    if b.enabled(BakeoffModel::Jev).await {
        return Err("B should see Jev off after one refresh".into());
    }
    if !b.enabled(BakeoffModel::Gemini).await {
        return Err("Gemini should stay on after the Jev switch-off".into());
    }
    Ok(())
}

/// The written config document holds exactly the three allowed fields.
#[tokio::test]
async fn cfg_1_switch_write_holds_only_allowed_fields() -> Result<(), String> {
    let store = Arc::new(InMemoryServerStore::new());
    seed(&store, cfg(true, true)).await?;
    let clock = Arc::new(VirtualClock::new(T0));
    let s = switches(
        EnvSwitches {
            gemini: true,
            jev: true,
        },
        Arc::clone(&store) as Arc<dyn ServerStore>,
        clock as Arc<dyn Clock>,
    );
    s.set(BakeoffModel::Jev, false)
        .await
        .map_err(|e| format!("{e:?}"))?;
    let read = store
        .config()
        .get_classifiers()
        .await
        .map_err(|e| format!("{e:?}"))?
        .ok_or_else(|| "document should exist".to_owned())?;
    let value = serde_json::to_value(read.record).map_err(|e| e.to_string())?;
    let obj = value
        .as_object()
        .ok_or_else(|| "record should be an object".to_owned())?;
    let keys: Vec<&str> = {
        let mut k: Vec<&str> = obj.keys().map(String::as_str).collect();
        k.sort_unstable();
        k
    };
    if keys != ["gemini_enabled", "jev_enabled", "updated_at"] {
        return Err(format!(
            "config document should hold exactly the three allowed fields, got {keys:?}"
        ));
    }
    Ok(())
}

/// When the env flag is off, `enabled` returns false without reading the store
/// (JEV-1) and without consuming an armed failure.
#[tokio::test]
async fn cfg_1_env_off_never_reads_store() -> Result<(), String> {
    let store = Arc::new(InMemoryServerStore::new());
    seed(&store, cfg(true, true)).await?;
    let clock = Arc::new(VirtualClock::new(T0));
    let s = switches(
        EnvSwitches {
            gemini: true,
            jev: false,
        },
        Arc::clone(&store) as Arc<dyn ServerStore>,
        clock as Arc<dyn Clock>,
    );
    store.fail_next(1);
    if s.enabled(BakeoffModel::Jev).await {
        return Err("env off should report Jev disabled".into());
    }
    // The armed failure was not consumed by `enabled`; the next read trips it.
    if store.config().get_classifiers().await.is_ok() {
        return Err("the armed store failure should still be pending".into());
    }
    Ok(())
}

/// A missing document means defaults-on (the admin has switched nothing off).
#[tokio::test]
async fn classifier_switches_missing_document_defaults_on() -> Result<(), String> {
    let store = Arc::new(InMemoryServerStore::new());
    let clock = Arc::new(VirtualClock::new(T0));
    let s = switches(
        EnvSwitches {
            gemini: true,
            jev: true,
        },
        Arc::clone(&store) as Arc<dyn ServerStore>,
        clock as Arc<dyn Clock>,
    );
    if !s.enabled(BakeoffModel::Gemini).await || !s.enabled(BakeoffModel::Jev).await {
        return Err("missing document should default both models on".into());
    }
    let snap = s.snapshot().await;
    if !snap.gemini || !snap.jev {
        return Err("missing document snapshot should be both on".into());
    }
    Ok(())
}

/// A store read error fails closed: both models stop rather than run blind.
#[tokio::test]
async fn classifier_switches_store_error_fails_closed() -> Result<(), String> {
    let store = Arc::new(InMemoryServerStore::new());
    seed(&store, cfg(true, true)).await?;
    let clock = Arc::new(VirtualClock::new(T0));
    let s = switches(
        EnvSwitches {
            gemini: true,
            jev: true,
        },
        Arc::clone(&store) as Arc<dyn ServerStore>,
        clock as Arc<dyn Clock>,
    );
    store.fail_next(1);
    if s.enabled(BakeoffModel::Gemini).await {
        return Err("store error should fail Gemini closed".into());
    }
    let snap = s.snapshot().await;
    if snap.gemini || snap.jev {
        return Err("store error snapshot should be both off".into());
    }
    Ok(())
}

/// A deciding store whose first `put` fails with `PreconditionFailed` to force
/// the retry path in `set`.
struct ContendingConfig {
    state: std::sync::Mutex<ContendState>,
}

#[derive(Default)]
struct ContendState {
    doc: Option<(ClassifiersConfig, u64)>,
    next: u64,
    first_put_fails: bool,
    put_calls: u32,
}

impl ContendingConfig {
    fn with_doc(doc: ClassifiersConfig) -> Self {
        Self {
            state: std::sync::Mutex::new(ContendState {
                doc: Some((doc, 1)),
                next: 2,
                first_put_fails: true,
                put_calls: 0,
            }),
        }
    }
    fn put_calls(&self) -> u32 {
        self.state
            .lock()
            .unwrap_or_else(|_| panic!("store poisoned"))
            .put_calls
    }
}

#[async_trait::async_trait]
impl ConfigRepo for ContendingConfig {
    async fn get_classifiers(&self) -> Result<Option<Versioned<ClassifiersConfig>>, StoreError> {
        let s = self
            .state
            .lock()
            .unwrap_or_else(|_| panic!("store poisoned"));
        Ok(s.doc.as_ref().map(|(c, v)| Versioned {
            record: c.clone(),
            version: Version(v.to_string()),
        }))
    }
    async fn put_classifiers(
        &self,
        cfg: &ClassifiersConfig,
        pre: Precondition,
    ) -> Result<Version, StoreError> {
        let mut s = self
            .state
            .lock()
            .unwrap_or_else(|_| panic!("store poisoned"));
        s.put_calls += 1;
        if s.first_put_fails {
            s.first_put_fails = false;
            return Err(StoreError::PreconditionFailed);
        }
        let existing = s.doc.as_ref().map(|(_, v)| v.to_string());
        match pre {
            Precondition::None => {}
            Precondition::MustNotExist => {
                if existing.is_some() {
                    return Err(StoreError::AlreadyExists);
                }
            }
            Precondition::MustExist => {
                if existing.is_none() {
                    return Err(StoreError::PreconditionFailed);
                }
            }
            Precondition::Matches(v) => {
                if existing.as_deref() != Some(v.0.as_str()) {
                    return Err(StoreError::PreconditionFailed);
                }
            }
        }
        let version = s.next;
        s.next += 1;
        s.doc = Some((cfg.clone(), version));
        Ok(Version(version.to_string()))
    }
}

/// Delegate every repo to a real `InMemoryServerStore` except `config`, which
/// is the contending repo, so `set`'s retry path is exercised deterministically.
struct ContendingStore {
    inner: InMemoryServerStore,
    config_repo: ContendingConfig,
}

impl ServerStore for ContendingStore {
    fn users(&self) -> &dyn UserRepo {
        self.inner.users()
    }
    fn mailboxes(&self) -> &dyn MailboxRepo {
        self.inner.mailboxes()
    }
    fn invites(&self) -> &dyn InviteRepo {
        self.inner.invites()
    }
    fn invite_requests(&self) -> &dyn InviteRequestRepo {
        self.inner.invite_requests()
    }
    fn jobs(&self) -> &dyn JobRepo {
        self.inner.jobs()
    }
    fn needs_attention(&self) -> &dyn NeedsAttentionRepo {
        self.inner.needs_attention()
    }
    fn sessions(&self) -> &dyn SessionRepo {
        self.inner.sessions()
    }
    fn classifier_eval(&self) -> &dyn ClassifierEvalRepo {
        self.inner.classifier_eval()
    }
    fn bakeoff_snapshots(&self) -> &dyn BakeoffSnapshotRepo {
        self.inner.bakeoff_snapshots()
    }
    fn config(&self) -> &dyn ConfigRepo {
        &self.config_repo
    }
    fn rate_limits(&self) -> &dyn RateLimitRepo {
        self.inner.rate_limits()
    }
}

/// `set` retries a `PreconditionFailed` write: the first put fails and the
/// retry succeeds.
#[tokio::test]
async fn classifier_switches_set_retries_on_conflict() -> Result<(), String> {
    let store = Arc::new(ContendingStore {
        inner: InMemoryServerStore::new(),
        config_repo: ContendingConfig::with_doc(cfg(true, true)),
    });
    let clock = Arc::new(VirtualClock::new(T0));
    let s = switches(
        EnvSwitches {
            gemini: true,
            jev: true,
        },
        store.clone() as Arc<dyn ServerStore>,
        clock as Arc<dyn Clock>,
    );
    let result = s
        .set(BakeoffModel::Jev, false)
        .await
        .map_err(|e| format!("{e:?}"))?;
    if result.jev || !result.gemini {
        return Err("retried set should produce Jev off, Gemini on".into());
    }
    if store.config_repo.put_calls() != 2 {
        return Err(format!(
            "set should retry the conflicting put once, got {} puts",
            store.config_repo.put_calls()
        ));
    }
    Ok(())
}
