//! T-906b: API-ADM-8 `GET /admin/experiments/classifier` and API-ADM-9
//! `PATCH /admin/experiments/classifier` (S7 5.13, S7 3.6, S7 6 "Experiment
//! admin changes"; S2 CL-04 AC1, AC8, AU-01 AC5; S5 `config/classifiers`,
//! CFG-1; ASVS V8.2.1, V16.3.3).
//!
//! An admin reads and changes the two kill switches without a deploy. Reads
//! need an admin; writes need an admin and a fresh Google sign-in (step-up,
//! [`SteppedUpAdmin`]), are limited to 50 per admin per day
//! (`policies::EXPERIMENT_ADMIN`) and go to the security log. The write holds
//! exactly the three allowed fields of `config/classifiers` (CFG-1) and
//! invalidates this instance's cache, so the next Feed request here sees the
//! change at once and other instances within a minute (CL-04 AC1).
//!
//! The bodies are parsed with [`ApiJson`], the strict JSON extractor every
//! other write route uses (S7 section 4: wrong content type, oversized body and
//! unknown fields are refused with a problem body).

use axum::extract::State;
use axum::Json;
use domain::UserId;
use obs::{Pseudonymiser, SecurityEvent};
use ports::store::{StoreCursor, MAX_PAGE};
use ports::{ClassifiersConfig, PageRequest, Precondition, StoreError};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::error::ApiError;
use crate::experiments::consent_is_current;
use crate::http::json::ApiJson;
use crate::http::request_id::RequestId;
use crate::limits::{policies, LimitSubject};
use crate::session::extract::{AdminSession, SteppedUpAdmin};
use crate::state::AppState;

/// One model's switch in the API-ADM-8 body (S7 5.13).
#[derive(Serialize)]
pub struct ModelSwitchDto {
    /// `gemini` or `jev`.
    pub model: &'static str,
    /// The wired classifier's ID (`gemini@<model>`, `jev@1.13.0`).
    pub classifier_id: String,
    /// The effective switch: the environment AND the document.
    pub enabled: bool,
    /// The document's `updated_at`, shared by both models (CFG-1 forbids a
    /// per-model timestamp); `null` until the document exists.
    #[serde(with = "time::serde::rfc3339::option")]
    pub changed_at: Option<OffsetDateTime>,
}

/// The API-ADM-8 / API-ADM-9 body (S7 5.13).
#[derive(Serialize)]
pub struct AdminClassifierDto {
    /// Gemini, then Jev.
    pub models: Vec<ModelSwitchDto>,
    /// Users whose consent is current: the bake-off's live population.
    pub participants: u64,
}

/// The API-ADM-9 request body.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PatchClassifier {
    /// One entry per model to change; at least one.
    pub models: Vec<PatchModel>,
}

/// One requested switch change.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PatchModel {
    /// Which model to change.
    pub model: ModelName,
    /// The new value; the effective switch also needs the environment on.
    pub enabled: bool,
}

/// A bake-off model, as the admin API names it (S7 5.13).
#[derive(Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ModelName {
    /// Gemini (Vertex AI).
    Gemini,
    /// Jev (`TypeSafe`).
    Jev,
}

impl ModelName {
    /// The name the API uses for this model.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Gemini => "gemini",
            Self::Jev => "jev",
        }
    }
}

/// `GET /api/v1/admin/experiments/classifier` (API-ADM-8).
///
/// # Errors
///
/// `ApiError::Forbidden` for a non-admin (the extractor logs and refuses);
/// `ApiError::Unauthenticated` when the admin's record is gone;
/// `ApiError::Internal` on a store failure.
pub async fn get_classifier(
    State(app): State<AppState>,
    _admin: AdminSession,
) -> Result<Json<AdminClassifierDto>, ApiError> {
    Ok(Json(admin_body(&app).await?))
}

/// `PATCH /api/v1/admin/experiments/classifier` (API-ADM-9).
///
/// # Errors
///
/// `ApiError::Forbidden` for a non-admin, `ApiError::StepUpRequired` without a
/// fresh Google sign-in (both from the extractors; the admin check runs first);
/// `ApiError::RateLimited` past the daily cap; `ApiError::InvalidRequest` for no
/// models or a repeated model; `ApiError::Internal` on a store failure.
pub async fn patch_classifier(
    State(app): State<AppState>,
    SteppedUpAdmin(admin): SteppedUpAdmin,
    request_id: RequestId,
    ApiJson(body): ApiJson<PatchClassifier>,
) -> Result<Json<AdminClassifierDto>, ApiError> {
    // 1. The admin and step-up checks are the extractors (AU-01 AC5, CL-04
    //    AC8): a missing step-up is `403 step_up_required`, a non-admin
    //    `403 forbidden` plus an `authz_failure` event.
    // 2. 50 changes per admin per day (S7 6).
    app.limits
        .check(
            &policies::EXPERIMENT_ADMIN,
            LimitSubject::User(&admin.user),
            request_id,
        )
        .await?;

    // 3. No change at all, or one model named twice, is a client error: the
    //    second entry would silently win.
    if body.models.is_empty() {
        return Err(invalid_models());
    }
    let mut seen = [false; 2];
    for change in &body.models {
        let slot = match change.model {
            ModelName::Gemini => 0,
            ModelName::Jev => 1,
        };
        if seen[slot] {
            return Err(invalid_models());
        }
        seen[slot] = true;
    }

    // 4. Read, apply, write, retrying once on a losing race.
    let now = app.ports.clock.now();
    write_switches(&app, &body.models, now).await?;

    // 6. This instance shows the change at once; every other instance picks it
    //    up within SWITCH_REFRESH.
    app.switches.invalidate();

    // 7. One security event per changed model, naming the admin
    //    pseudonymously (S6 7, ASVS V16.3.3).
    for change in &body.models {
        kill_switch_event(&app, &admin.user, change.model, change.enabled, request_id);
    }

    // 8. The API-ADM-8 body.
    Ok(Json(admin_body(&app).await?))
}

/// A refused `models` list: empty, or one model named twice (S7 4).
fn invalid_models() -> ApiError {
    ApiError::InvalidRequest {
        fields: vec!["models".to_owned()],
    }
}

/// The API-ADM-8 body: both models, their effective switch and the document's
/// timestamp, joined with the count of users whose consent is current.
async fn admin_body(app: &AppState) -> Result<AdminClassifierDto, ApiError> {
    let document = app.ports.store.config().get_classifiers().await?;
    let changed_at = document.as_ref().map(|v| v.record.updated_at);
    let (gemini, jev) = app.switches.current(&app.ports).await;
    Ok(AdminClassifierDto {
        models: vec![
            ModelSwitchDto {
                model: ModelName::Gemini.as_str(),
                classifier_id: classifier_id(app, ModelName::Gemini),
                enabled: gemini,
                changed_at,
            },
            ModelSwitchDto {
                model: ModelName::Jev.as_str(),
                classifier_id: classifier_id(app, ModelName::Jev),
                enabled: jev,
                changed_at,
            },
        ],
        participants: participants(app).await?,
    })
}

/// The ID of the wired classifier for a model, or `<model>@unavailable` when
/// the process did not wire that model (a missing key, say).
fn classifier_id(app: &AppState, model: ModelName) -> String {
    let wired = match model {
        ModelName::Gemini => app.classifiers.gemini.as_ref(),
        ModelName::Jev => app.classifiers.jev.as_ref(),
    };
    wired.map_or_else(|| format!("{}@unavailable", model.as_str()), |c| c.id().0)
}

/// How many users have a current consent (S7 5.13). Trial scale is small, so
/// one bounded page walk is enough (`[DEFAULT]`).
async fn participants(app: &AppState) -> Result<u64, ApiError> {
    let mut count = 0_u64;
    let mut after: Option<StoreCursor> = None;
    loop {
        let page = app
            .ports
            .store
            .users()
            .list(PageRequest {
                limit: MAX_PAGE,
                after: after.clone(),
            })
            .await?;
        for item in &page.items {
            if consent_is_current(&item.record) {
                count = count.saturating_add(1);
            }
        }
        match page.next {
            Some(next) if !page.items.is_empty() => after = Some(next),
            _ => return Ok(count),
        }
    }
}

/// Apply the requested changes to `config/classifiers`, and only those three
/// fields (CFG-1).
///
/// A missing document starts from both switches off. A precondition failure
/// re-reads and re-applies once, then gives up rather than loop.
///
/// # Errors
///
/// `ApiError::Internal` when a competing writer keeps winning or the store is
/// failing.
async fn write_switches(
    app: &AppState,
    changes: &[PatchModel],
    now: OffsetDateTime,
) -> Result<(), ApiError> {
    for _ in 0..2 {
        let current = app.ports.store.config().get_classifiers().await?;
        let (mut gemini, mut jev) = match &current {
            Some(v) => (v.record.gemini_enabled, v.record.jev_enabled),
            None => (false, false),
        };
        for change in changes {
            match change.model {
                ModelName::Gemini => gemini = change.enabled,
                ModelName::Jev => jev = change.enabled,
            }
        }
        let document = ClassifiersConfig {
            gemini_enabled: gemini,
            jev_enabled: jev,
            updated_at: now,
        };
        let precondition = match &current {
            Some(v) => Precondition::Matches(v.version.clone()),
            None => Precondition::MustNotExist,
        };
        match app
            .ports
            .store
            .config()
            .put_classifiers(&document, precondition)
            .await
        {
            Ok(_) => return Ok(()),
            Err(StoreError::PreconditionFailed) => {}
            Err(e) => return Err(e.into()),
        }
    }
    Err(ApiError::Internal)
}

/// One `kill_switch_change` security event (S6 7, ASVS V16.3.3).
///
/// The model and the new value ride in the outcome code: S5's log fields have
/// no free slot for them (`provider` is a mail provider, `method` is an
/// unsubscribe method), and every logged value comes from a closed registry.
fn kill_switch_event(
    app: &AppState,
    admin: &UserId,
    model: ModelName,
    enabled: bool,
    request_id: RequestId,
) {
    obs::security_event(&SecurityEvent {
        action: "kill_switch_change",
        outcome: change_outcome(model, enabled),
        user: Some(Pseudonymiser::new(app.config.rate_key.clone()).pseudo_id(&admin.0)),
        request_id: Some(request_id.0),
        amr: None,
        provider: None,
        method: None,
    });
}

/// The registered outcome code for a change: the model and its new value.
fn change_outcome(model: ModelName, enabled: bool) -> &'static str {
    match (model, enabled) {
        (ModelName::Gemini, true) => "gemini_enabled",
        (ModelName::Gemini, false) => "gemini_disabled",
        (ModelName::Jev, true) => "jev_enabled",
        (ModelName::Jev, false) => "jev_disabled",
    }
}
