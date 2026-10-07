//! T-902: API-EXP-1 `GET /me/experiments` and API-EXP-2 `PUT /me/experiments`
//! (S7 5.13; S2 CL-02 AC2, AC3; S5 EXP-4; ASVS V2.4.1, V16.3.3).
//!
//! GET reads the user's consent state. PUT changes it: opting in records the
//! consent text version, opting out clears both fields and deletes the user's
//! `classifier_eval` records before the response returns.

use axum::extract::State;
use axum::Json;
use domain::UserId;
use obs::{Pseudonymiser, SecurityEvent};
use ports::{Precondition, StoreError, UserPseudoId, UserRecord};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::error::ApiError;
use crate::experiments::{consent_is_current, CURRENT_CONSENT_VERSION};
use crate::http::json::ApiJson;
use crate::http::request_id::RequestId;
use crate::limits::{policies, LimitSubject};
use crate::session::extract::AuthedSession;
use crate::state::AppState;

/// The API-EXP-1 / API-EXP-2 response body.
#[derive(Serialize)]
pub struct ExperimentsDto {
    pub classifier_bakeoff: BakeoffConsentDto,
}

/// The bake-off consent state.
#[derive(Serialize)]
pub struct BakeoffConsentDto {
    /// False while both models are switched off.
    pub available: bool,
    /// True only for a current consent; an old version reads as opted out.
    pub opted_in: bool,
    /// The stored consent text version, verbatim (so the app can tell an old
    /// consent from none); `null` when the record has none.
    pub consent_version: Option<String>,
    /// The version this server accepts.
    pub current_consent_version: &'static str,
    /// When the stored consent was given; `null` when the record has none.
    #[serde(with = "time::serde::rfc3339::option")]
    pub opted_in_at: Option<OffsetDateTime>,
}

/// The API-EXP-2 request body.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PutExperiments {
    pub classifier_bakeoff: PutBakeoff,
}

/// One consent change.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PutBakeoff {
    pub opted_in: bool,
    pub consent_version: Option<String>,
}

/// `GET /api/v1/me/experiments` (API-EXP-1).
///
/// # Errors
///
/// `ApiError::Unauthenticated` when the user record is gone; `Internal` on a
/// store failure.
pub async fn get_experiments(
    State(app): State<AppState>,
    s: AuthedSession,
) -> Result<Json<ExperimentsDto>, ApiError> {
    let user = load_user(&app, &s.user).await?;
    let switches = switches(&app).await?;
    Ok(Json(dto(&user, switches)))
}

/// `PUT /api/v1/me/experiments` (API-EXP-2).
///
/// # Errors
///
/// `RateLimited` over ten changes a day; `ConsentOutdated` for an opt-in with an
/// old version; `ExperimentUnavailable` for an opt-in while both models are
/// off; `ProviderUnavailable` when an opt-out cannot delete the evaluation
/// records (the consent is already cleared, so a retry finishes the job).
pub async fn put_experiments(
    State(app): State<AppState>,
    s: AuthedSession,
    request_id: RequestId,
    ApiJson(body): ApiJson<PutExperiments>,
) -> Result<Json<ExperimentsDto>, ApiError> {
    // 1. Ten changes per user per day (V2.4.1, S7 6).
    app.limits
        .check(
            &policies::EXPERIMENT_OPT,
            LimitSubject::User(&s.user),
            request_id,
        )
        .await?;
    let switches = switches(&app).await?;

    if body.classifier_bakeoff.opted_in {
        // 2. Opt in: the exact current version, and a model switched on.
        if body.classifier_bakeoff.consent_version.as_deref() != Some(CURRENT_CONSENT_VERSION) {
            return Err(ApiError::ConsentOutdated);
        }
        if !(switches.0 || switches.1) {
            return Err(ApiError::ExperimentUnavailable);
        }
        let now = app.ports.clock.now();
        update_user(&app, &s.user, |record| {
            record.experiments_consent_version = Some(CURRENT_CONSENT_VERSION.to_owned());
            record.experiments_opted_in_at = Some(now);
        })
        .await?;
        consent_event(&app, "experiments_opt_in", &s.user);
    } else {
        // 3. Opt out: clear first, so the gate closes on the next read, then
        //    delete the evaluation records before the response (CL-02 AC3).
        update_user(&app, &s.user, |record| {
            record.experiments_consent_version = None;
            record.experiments_opted_in_at = None;
        })
        .await?;
        let _deleted = app
            .ports
            .store
            .classifier_eval()
            .delete_for_users(&pseudo_ids(&app, &s.user))
            .await
            .map_err(|_| ApiError::ProviderUnavailable {
                mailbox_id: None,
                retry_after_s: None,
            })?;
        consent_event(&app, "experiments_opt_out", &s.user);
    }

    let user = load_user(&app, &s.user).await?;
    Ok(Json(dto(&user, switches)))
}

/// The DTO for a user record and the effective switch state.
fn dto(user: &UserRecord, switches: (bool, bool)) -> ExperimentsDto {
    ExperimentsDto {
        classifier_bakeoff: BakeoffConsentDto {
            available: switches.0 || switches.1,
            opted_in: consent_is_current(user),
            consent_version: user.experiments_consent_version.clone(),
            current_consent_version: CURRENT_CONSENT_VERSION,
            opted_in_at: user.experiments_opted_in_at,
        },
    }
}

/// Load the signed-in user's record.
async fn load_user(app: &AppState, user: &UserId) -> Result<UserRecord, ApiError> {
    app.ports
        .store
        .users()
        .get(user)
        .await?
        .map(|v| v.record)
        .ok_or(ApiError::Unauthenticated)
}

/// The effective switch state `(gemini, jev)`. Until T-906b's reader is wired
/// here, read `config/classifiers` directly; a missing document means both off
/// (fail closed).
async fn switches(app: &AppState) -> Result<(bool, bool), ApiError> {
    match app.ports.store.config().get_classifiers().await {
        Ok(Some(v)) => Ok((v.record.gemini_enabled, v.record.jev_enabled)),
        Ok(None) => Ok((false, false)),
        Err(_) => Err(ApiError::Internal),
    }
}

/// Apply a change to the user record with a compare-and-set, retrying once on
/// a losing race (`PreconditionFailed`).
async fn update_user<F>(app: &AppState, user: &UserId, change: F) -> Result<(), ApiError>
where
    F: Fn(&mut UserRecord),
{
    for _ in 0..2 {
        let Some(versioned) = app.ports.store.users().get(user).await? else {
            return Err(ApiError::Unauthenticated);
        };
        let mut record = versioned.record;
        change(&mut record);
        match app
            .ports
            .store
            .users()
            .put(&record, Precondition::Matches(versioned.version))
            .await
        {
            Ok(_) => return Ok(()),
            Err(StoreError::PreconditionFailed) => {}
            Err(e) => return Err(e.into()),
        }
    }
    Err(ApiError::Internal)
}

/// Every pseudonymous ID the user's `classifier_eval` rows could be under.
///
/// There is one log key version today, so this is a single ID; a log-key
/// rotation adds the old keys here in one place (the same set the log lines
/// were written under).
fn pseudo_ids(app: &AppState, user: &UserId) -> Vec<UserPseudoId> {
    let pseudo = Pseudonymiser::new(app.config.rate_key.clone()).pseudo_id(&user.0);
    vec![UserPseudoId(pseudo.as_str().to_owned())]
}

/// One pseudonymous consent event; never an address, token or version string
/// (the log allowlist carries no free field for the version).
fn consent_event(app: &AppState, action: &'static str, user: &UserId) {
    obs::security_event(&SecurityEvent {
        action,
        outcome: "success",
        user: Some(Pseudonymiser::new(app.config.rate_key.clone()).pseudo_id(&user.0)),
        request_id: None,
        amr: None,
        provider: None,
        method: None,
    });
}
