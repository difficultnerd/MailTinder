//! The classifier kill switches: start-up environment flags combined with the
//! `config/classifiers` Firestore document, re-read at most once a minute.
//!
//! An admin can turn Gemini or Jev off without a deploy (CL-04 AC1, CFG-1).
//! The env flag is the start-up gate (JEV-1: off means zero calls); the
//! document flag is the live gate. Refresh is lazy on read, driven by the
//! `Clock`, so tests advance a virtual clock instead of waiting.

use std::sync::Arc;

use ports::store::{ClassifiersConfig, Precondition, ServerStore, StoreError};
use ports::Clock;
use time::{Duration, OffsetDateTime};

/// How often the Firestore document is re-read (S4 5.7 "checked every minute").
pub const CONFIG_REFRESH: Duration = Duration::seconds(60);

/// A bake-off model.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BakeoffModel {
    Gemini,
    Jev,
}

/// The start-up environment switches.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EnvSwitches {
    pub gemini: bool,
    pub jev: bool,
}

impl EnvSwitches {
    /// Reads `CLASSIFIER_GEMINI_ENABLED` and `CLASSIFIER_JEV_ENABLED`.
    ///
    /// The parse is strict: `"true"` or `"1"` is on; unset or anything else
    /// (including `"TRUE "` with a space) is off.
    pub fn from_env() -> Self {
        Self {
            gemini: env_on("CLASSIFIER_GEMINI_ENABLED"),
            jev: env_on("CLASSIFIER_JEV_ENABLED"),
        }
    }
}

fn env_on(name: &str) -> bool {
    match std::env::var(name) {
        Ok(v) => v == "true" || v == "1",
        Err(_) => false,
    }
}

/// The effective switch state from the document.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SwitchState {
    pub gemini: bool,
    pub jev: bool,
}

/// Combines the env flags with the `config/classifiers` document.
pub struct ClassifierSwitches {
    env: EnvSwitches,
    store: Arc<dyn ServerStore>,
    clock: Arc<dyn Clock>,
    state: tokio::sync::Mutex<Option<(SwitchState, OffsetDateTime)>>,
}

impl ClassifierSwitches {
    pub fn new(env: EnvSwitches, store: Arc<dyn ServerStore>, clock: Arc<dyn Clock>) -> Self {
        Self {
            env,
            store,
            clock,
            state: tokio::sync::Mutex::new(None),
        }
    }

    /// The effective switch for a model: env flag AND document flag, refreshed
    /// when the cached read is `CONFIG_REFRESH` old.
    pub async fn enabled(&self, model: BakeoffModel) -> bool {
        let env_flag = match model {
            BakeoffModel::Gemini => self.env.gemini,
            BakeoffModel::Jev => self.env.jev,
        };
        if !env_flag {
            // JEV-1: start-up switch off means zero calls, no store read.
            return false;
        }
        let state = self.refresh().await;
        match model {
            BakeoffModel::Gemini => state.gemini,
            BakeoffModel::Jev => state.jev,
        }
    }

    /// The current effective switch state.
    pub async fn snapshot(&self) -> SwitchState {
        self.refresh().await
    }

    /// Admin write (API-ADM-9 in T-906 calls it after step-up). Writes only
    /// the three allowed fields and refreshes this instance's cache
    /// immediately.
    pub async fn set(&self, model: BakeoffModel, enabled: bool) -> Result<SwitchState, StoreError> {
        let now = self.clock.now();
        let mut guard = self.state.lock().await;
        let current = match guard.as_ref() {
            Some((s, _)) => *s,
            None => self.read_document().await?,
        };
        let mut next = current;
        match model {
            BakeoffModel::Gemini => next.gemini = enabled,
            BakeoffModel::Jev => next.jev = enabled,
        }
        let cfg = ClassifiersConfig {
            gemini_enabled: next.gemini,
            jev_enabled: next.jev,
            updated_at: now,
        };
        self.write_with_retry(cfg, 3).await?;
        *guard = Some((next, now));
        Ok(next)
    }

    /// Refresh the cached document state if it is stale, returning the state.
    async fn refresh(&self) -> SwitchState {
        let mut guard = self.state.lock().await;
        let now = self.clock.now();
        let needs_refresh = match guard.as_ref() {
            None => true,
            Some((_, fetched_at)) => now - *fetched_at >= CONFIG_REFRESH,
        };
        if needs_refresh {
            let state = match self.read_document().await {
                Ok(s) => s,
                Err(_) => SwitchState {
                    gemini: false,
                    jev: false,
                },
            };
            *guard = Some((state, now));
        }
        guard.as_ref().map(|(s, _)| *s).unwrap_or(SwitchState {
            gemini: false,
            jev: false,
        })
    }

    /// Read the document, mapping `Ok(None)` to defaults-on and `Err` to
    /// fail-closed (both off).
    async fn read_document(&self) -> Result<SwitchState, StoreError> {
        match self.store.config().get_classifiers().await {
            Ok(Some(v)) => Ok(SwitchState {
                gemini: v.record.gemini_enabled,
                jev: v.record.jev_enabled,
            }),
            Ok(None) => Ok(SwitchState {
                gemini: true,
                jev: true,
            }),
            Err(e) => Err(e),
        }
    }

    /// Write the document, retrying on `PreconditionFailed` up to `attempts`
    /// times with a fresh read each time.
    async fn write_with_retry(
        &self,
        cfg: ClassifiersConfig,
        attempts: u32,
    ) -> Result<(), StoreError> {
        let mut remaining = attempts;
        loop {
            let pre = match self.store.config().get_classifiers().await? {
                Some(v) => Precondition::Matches(v.version),
                None => Precondition::MustNotExist,
            };
            match self.store.config().put_classifiers(&cfg, pre).await {
                Ok(_) => return Ok(()),
                Err(StoreError::PreconditionFailed) if remaining > 0 => {
                    remaining -= 1;
                }
                Err(e) => return Err(e),
            }
        }
    }
}
