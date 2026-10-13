//! T-906b: the classifier kill switches — the start-up environment flags AND
//! the `config/classifiers` document, re-read at most once a minute (S4 5.7,
//! S5 `config/classifiers`; S2 CL-04 AC1, CFG-1).
//!
//! The effective switch for a model is `env AND document`: the environment can
//! always turn a model off, and the document can never turn on a model the
//! environment turned off. A missing document is no configuration, and an
//! unreadable store fails closed once the last good read is older than
//! [`SWITCH_STALE_MAX`], so a broken configuration never opens the gate (S2
//! CL-02 AC2, JEV-1).
//!
//! Reads are lazy on [`ClassifierSwitches::current`] and driven by the `Clock`,
//! so a test advances a virtual clock instead of waiting. The cache is per
//! instance: an admin write calls [`ClassifierSwitches::invalidate`] so the
//! writing instance sees its own change at once, and every other instance picks
//! it up within [`SWITCH_REFRESH`].

use std::sync::Mutex;

use obs::OpLog;
use ports::Ports;
use time::{Duration, OffsetDateTime};

/// How often `config/classifiers` is re-read (S4 5.7 "checked every minute").
pub const SWITCH_REFRESH: Duration = Duration::seconds(60);

/// `[DEFAULT]` for how long a good read stays usable while the store keeps
/// failing; past this the switches fail closed.
pub const SWITCH_STALE_MAX: Duration = Duration::minutes(5);

/// The start-up environment variable that gates Gemini (S4 5.7).
pub const GEMINI_ENV: &str = "CLASSIFIER_GEMINI_ENABLED";

/// The start-up environment variable that gates Jev (S4 5.7).
pub const JEV_ENV: &str = "CLASSIFIER_JEV_ENABLED";

/// The start-up switches, read once at boot.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct EnvSwitches {
    /// May the process call Gemini at all?
    pub gemini: bool,
    /// May the process call Jev at all?
    pub jev: bool,
}

impl EnvSwitches {
    /// The environment at start-up. Only the exact string `true` enables a
    /// model: unset, `1`, `TRUE` and every other value mean off
    /// (`[DEFAULT: fail closed]`).
    #[must_use]
    pub fn from_env() -> Self {
        Self::from_lookup(|name| std::env::var(name).ok())
    }

    /// [`Self::from_env`] over an injected lookup, so the parse is testable
    /// without touching the process environment.
    #[must_use]
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Self {
        Self {
            gemini: is_on(lookup(GEMINI_ENV).as_deref()),
            jev: is_on(lookup(JEV_ENV).as_deref()),
        }
    }
}

/// The strict parse: only `"true"` is on.
fn is_on(value: Option<&str>) -> bool {
    value == Some("true")
}

/// One cached document read: the two switches, when it was taken, and when the
/// store last answered at all (for [`SWITCH_STALE_MAX`]).
#[derive(Clone, Copy, Debug)]
struct Cached {
    gemini: bool,
    jev: bool,
    read_at: OffsetDateTime,
    last_ok_at: OffsetDateTime,
}

/// The effective kill switches: the start-up environment AND
/// `config/classifiers`.
pub struct ClassifierSwitches {
    env: EnvSwitches,
    /// The cached document value. `None` until the first read; `std::sync` so
    /// [`Self::invalidate`] stays a plain synchronous call, and the lock is
    /// never held across an `await`.
    cache: Mutex<Option<Cached>>,
}

impl ClassifierSwitches {
    /// A reader with no cached document; the first [`Self::current`] reads it.
    #[must_use]
    pub fn new(env: EnvSwitches) -> Self {
        Self {
            env,
            cache: Mutex::new(None),
        }
    }

    /// The effective `(gemini, jev)` for this request.
    ///
    /// Uses the cached document while it is younger than [`SWITCH_REFRESH`];
    /// otherwise it re-reads `config/classifiers`. A read failure keeps the last
    /// good value while that is younger than [`SWITCH_STALE_MAX`], and fails
    /// closed after that. Never returns an error: the gate is closed, not
    /// broken, when the store is down.
    pub async fn current(&self, ports: &Ports) -> (bool, bool) {
        let now = ports.clock.now();
        let doc = match self.cached(now) {
            Some(doc) => doc,
            None => self.read(ports, now).await,
        };
        (self.env.gemini && doc.0, self.env.jev && doc.1)
    }

    /// Drop the cache, so the next [`Self::current`] re-reads the document.
    ///
    /// API-ADM-9 calls this after a write: the writing instance shows the
    /// admin's change at once instead of up to [`SWITCH_REFRESH`] later.
    pub fn invalidate(&self) {
        if let Ok(mut guard) = self.cache.lock() {
            *guard = None;
        }
    }

    /// The cached document value, when it is still fresh.
    fn cached(&self, now: OffsetDateTime) -> Option<(bool, bool)> {
        let guard = self.cache.lock().ok()?;
        let cached = guard.as_ref()?;
        (now - cached.read_at < SWITCH_REFRESH).then_some((cached.gemini, cached.jev))
    }

    /// Read the document, caching a success and falling back on a failure.
    async fn read(&self, ports: &Ports, now: OffsetDateTime) -> (bool, bool) {
        match ports.store.config().get_classifiers().await {
            Ok(Some(v)) => {
                let doc = (v.record.gemini_enabled, v.record.jev_enabled);
                self.remember(doc, now);
                doc
            }
            // No document is no configuration: both off, and a good read (the
            // store answered), so it is cached like any other answer.
            Ok(None) => {
                self.remember((false, false), now);
                (false, false)
            }
            Err(_) => self.stale(now),
        }
    }

    /// The last good value while it is younger than [`SWITCH_STALE_MAX`], else
    /// `(false, false)` with one `classifier_config_unavailable` line (S6 7).
    fn stale(&self, now: OffsetDateTime) -> (bool, bool) {
        let last = self.cache.lock().ok().and_then(|guard| *guard);
        match last {
            Some(c) if now - c.last_ok_at < SWITCH_STALE_MAX => (c.gemini, c.jev),
            _ => {
                obs::op_log(&OpLog {
                    op: "classifier_config_unavailable",
                    outcome: "unreadable",
                    status: None,
                    latency_ms: None,
                });
                (false, false)
            }
        }
    }

    /// Cache a document read (the store answered, so this is a good read too).
    fn remember(&self, doc: (bool, bool), now: OffsetDateTime) {
        if let Ok(mut guard) = self.cache.lock() {
            *guard = Some(Cached {
                gemini: doc.0,
                jev: doc.1,
                read_at: now,
                last_ok_at: now,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    //! Unit tests for the reader itself (`S10 9.2` JEV-1's kill-switch half).

    use super::{ClassifierSwitches, EnvSwitches};
    use ports::{ClassifiersConfig, Clock, Precondition, ServerStore};
    use time::Duration;

    /// Only the exact string `true` enables a model at start-up.
    #[test]
    fn switches_env_only_exact_true_enables() {
        let read = |name: &str| match name {
            "CLASSIFIER_GEMINI_ENABLED" => Some("true".to_owned()),
            "CLASSIFIER_JEV_ENABLED" => Some("TRUE".to_owned()),
            _ => None,
        };
        let env = EnvSwitches::from_lookup(read);
        assert!(env.gemini, "the exact string true is on");
        assert!(!env.jev, "TRUE is not true");

        let one = EnvSwitches::from_lookup(|name| {
            (name == "CLASSIFIER_JEV_ENABLED").then(|| "1".to_owned())
        });
        assert!(!one.jev, "1 is off: fail closed");
        assert!(!EnvSwitches::from_lookup(|_| None).gemini, "unset is off");
    }

    /// CFG-1 / JEV-1: no document means no model is on.
    #[tokio::test]
    async fn switches_missing_doc_is_off() -> Result<(), Box<dyn std::error::Error>> {
        let (ports, fakes) = testkit::fake_ports();
        assert!(
            fakes.store.config().get_classifiers().await?.is_none(),
            "the fixture holds no config document"
        );
        let switches = ClassifierSwitches::new(EnvSwitches {
            gemini: true,
            jev: true,
        });
        assert_eq!(switches.current(&ports).await, (false, false));
        Ok(())
    }

    /// JEV-1: while the store is failing the last good read holds, and past
    /// [`super::SWITCH_STALE_MAX`] the switches fail closed.
    #[tokio::test]
    async fn switches_fail_closed_after_stale_max() -> Result<(), Box<dyn std::error::Error>> {
        let (ports, fakes) = testkit::fake_ports();
        fakes
            .store
            .config()
            .put_classifiers(
                &ClassifiersConfig {
                    gemini_enabled: true,
                    jev_enabled: true,
                    updated_at: fakes.clock.now(),
                },
                Precondition::MustNotExist,
            )
            .await?;
        let switches = ClassifierSwitches::new(EnvSwitches {
            gemini: true,
            jev: true,
        });
        assert_eq!(switches.current(&ports).await, (true, true));

        // The store now fails; the last good read still holds while it is
        // younger than SWITCH_STALE_MAX.
        fakes.store.fail_next(1);
        fakes.clock.advance(Duration::minutes(4));
        assert_eq!(
            switches.current(&ports).await,
            (true, true),
            "a good read inside the stale window still counts"
        );

        // Five minutes without a good read: fail closed.
        fakes.store.fail_next(1);
        fakes.clock.advance(Duration::minutes(1));
        assert_eq!(
            switches.current(&ports).await,
            (false, false),
            "past SWITCH_STALE_MAX the switches fail closed"
        );
        Ok(())
    }
}
