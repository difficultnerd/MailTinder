//! T-902: the Experiments consent half of the bake-off gate (S2 CL-02, S7 5.13).
//!
//! `consent_is_current` answers "has this user agreed to the current consent
//! text"; `bakeoff_gate` combines that with each model's kill switch. The Feed
//! (T-901) calls `bakeoff_gate` so no request reaches Gemini or Jev without a
//! current consent (CL-02 AC2, JEV-1).

use ports::UserRecord;

/// The consent text version the API accepts (S7 5.13). Bump in the same change
/// that edits the S9 7.8 wording, so old versions read as opted out.
pub const CURRENT_CONSENT_VERSION: &str = "2026-10-03";

/// Opt-in changes allowed per user per day (S7 6 "Experiment opt-in changes")
/// `[TUNABLE]`. This mirrors the `experiment_opt` rate policy; the test suite
/// pins the two together so they cannot drift.
pub const OPT_CHANGES_PER_DAY: u32 = 10;

/// The bake-off gate, computed per request: one flag per model.
///
/// This is T-901's `BakeoffGate` shape. T-902 defines it here because T-901's
/// classify module has not landed yet; T-201b/T-901 imports this once it does,
/// rather than defining a second type.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct BakeoffGate {
    /// May this request call Gemini?
    pub gemini: bool,
    /// May this request call Jev?
    pub jev: bool,
}

impl BakeoffGate {
    /// True when at least one model is due to run.
    #[must_use]
    pub fn is_open(self) -> bool {
        self.gemini || self.jev
    }
}

/// True when the user consented to the *current* consent text: the stored
/// version equals [`CURRENT_CONSENT_VERSION`] and `opted_in_at` is set. An old
/// version counts as opted out (S7 5.13); so does a record with neither field.
#[must_use]
pub fn consent_is_current(user: &UserRecord) -> bool {
    user.experiments_consent_version.as_deref() == Some(CURRENT_CONSENT_VERSION)
        && user.experiments_opted_in_at.is_some()
}

/// Consent AND switches. `switches` is `(gemini_enabled, jev_enabled)` from the
/// switch reader (T-906b). A model is open only when the user has current
/// consent and that model's switch is on (CL-02 AC2).
#[must_use]
pub fn bakeoff_gate(user: &UserRecord, switches: (bool, bool)) -> BakeoffGate {
    let consent = consent_is_current(user);
    BakeoffGate {
        gemini: consent && switches.0,
        jev: consent && switches.1,
    }
}
