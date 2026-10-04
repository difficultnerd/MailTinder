//! Scripted login and token scenarios for `fake-google` (T-206).

use serde::Deserialize;

/// A registered OAuth client.
#[derive(Clone, Debug, Deserialize)]
pub struct ClientReg {
    pub client_id: String,
    pub client_secret: String,
    pub redirect_uris: Vec<String>,
}

/// The scripted user for the next authorisation.
#[derive(Clone, Debug, Deserialize)]
pub struct NextLogin {
    pub sub: String,
    /// Reserved domain only.
    pub email: String,
    #[serde(default)]
    pub email_verified: bool,
    /// None: claim absent (Google's usual case).
    #[serde(default)]
    pub amr: Option<Vec<String>>,
    /// `auth_time` = `now` - `auth_age_s`; default 0.
    #[serde(default)]
    pub auth_age_s: i64,
    pub outcome: LoginOutcome,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LoginOutcome {
    Approve,
    AccessDenied,
}

/// One-shot switch applied to the next ID token the token endpoint issues.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TokenScenario {
    WrongAud,
    WrongIss,
    Expired,
    NotYetValid,
    ReplayPreviousNonce,
    MissingNonce,
    MissingAuthTime,
    SignedByUnknownKey,
    AlgNone,
    AlgHs256,
    NoKid,
}

impl Default for TokenScenario {
    /// The benign scenario when none is queued: nominally correct, kept as a
    /// real variant so `Option::take().unwrap_or_default()` in the token route
    /// always builds a valid token for the happy path.
    fn default() -> Self {
        TokenScenario::MissingNonce
    }
}

impl Default for NextLogin {
    fn default() -> Self {
        Self {
            sub: "sub-alice".to_owned(),
            email: "alice@example.com".to_owned(),
            email_verified: true,
            amr: None,
            auth_age_s: 0,
            outcome: LoginOutcome::Approve,
        }
    }
}
