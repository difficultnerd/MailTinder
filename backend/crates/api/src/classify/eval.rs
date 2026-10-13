//! The `classifier_eval` record (T-906a; S4 5.6, S7 5.13, CL-03, BAKE-6, EXP-1).
//!
//! One record per labelled swipe: the header-rules result, both models'
//! predictions (a `Some` with an `error_code` is a failed call, CL-03 AC2), the
//! swipe direction, the undo status and the time to swipe, plus the coarse
//! segment buckets and the method versions. Nothing that identifies the message
//! or its content: the record is built field by field from the sealed
//! classification payload, never by copying a struct wholesale (EXP-1).
//!
//! The record is written after the swipe's provider change has landed, and a
//! failed write never fails the swipe: the user's action matters more than the
//! experiment. Undo flips `outcome.undone` on the same record; nothing here
//! stores a label, which T-907's `label_for` derives in the report.
#![allow(clippy::module_name_repetitions)]

use domain::{SwipeAction, UserId};
use obs::Pseudonymiser;
use ports::{
    ClassifierEvalRecord, EvalId, EvalOutcome, HeaderRulesResult, Precondition, StoreError,
    SwipeDirection, UserPseudoId,
};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use crate::classify::payload::BakeoffPayload;
use crate::error::ApiError;
use crate::experiments::consent_is_current;
use crate::routes::feed::ClassificationPayload;
use crate::session::extract::AuthedSession;
use crate::state::AppState;

/// `[TUNABLE]` how long a `classifier_eval` record lives (S4 5.6, S5).
pub const EVAL_TTL_DAYS: i64 = 180;

/// `[DEFAULT]` the fixed namespace for eval IDs. Different from the job and
/// rule namespaces, so the same (user, `Idempotency-Key`) never gives a job ID
/// equal to an eval ID.
pub const EVAL_NAMESPACE: Uuid = uuid::uuid!("6f0c8a52-3c1e-5d7a-9b1e-2a4c6e8f0a13");

/// The sealed bake-off payload a swipe hands to [`write_eval`], with the
/// `Idempotency-Key` the record's ID is derived from. `None` at the call site
/// means the classification token did not open, so there is nothing to record
/// (S7 5.5).
pub struct EvalInput<'a> {
    pub payload: &'a ClassificationPayload,
    pub idempotency_key: Uuid,
}

/// The eval ID of a swipe: UUID v5(`EVAL_NAMESPACE`, user bytes ++ key bytes).
/// Never derived from the message ID (S7 5.13, EXP-1).
#[must_use]
pub fn eval_id_for(user: &UserId, idempotency_key: &Uuid) -> EvalId {
    let mut bytes = Vec::with_capacity(32);
    bytes.extend_from_slice(user.0.as_bytes());
    bytes.extend_from_slice(idempotency_key.as_bytes());
    EvalId(Uuid::new_v5(&EVAL_NAMESPACE, &bytes))
}

/// The bake-off direction of a swipe action (S3, S7 5.13): keep is `Right`,
/// reject is `Left`, file is `Up`, skip is `Down`.
#[must_use]
pub fn direction_for(action: SwipeAction) -> SwipeDirection {
    match action {
        SwipeAction::Keep => SwipeDirection::Right,
        SwipeAction::Reject => SwipeDirection::Left,
        SwipeAction::File { .. } => SwipeDirection::Up,
        SwipeAction::Skip => SwipeDirection::Down,
    }
}

/// Build the record for one swipe. Every field is copied explicitly: the
/// payload also carries the mailbox and message IDs, which never reach the
/// record.
#[must_use]
pub fn build_eval_record(
    user_pseudo_id: UserPseudoId,
    payload: &ClassificationPayload,
    bakeoff: &BakeoffPayload,
    direction: SwipeDirection,
    now: OffsetDateTime,
    eval_id: EvalId,
) -> ClassifierEvalRecord {
    ClassifierEvalRecord {
        eval_id,
        user_pseudo_id,
        created_at: now,
        expires_at: now + Duration::days(EVAL_TTL_DAYS),
        header_rules: HeaderRulesResult {
            class: payload.header_rules.class,
            score: payload.header_rules.bulk_score,
        },
        gemini: bakeoff.gemini.clone(),
        jev: bakeoff.jev.clone(),
        outcome: EvalOutcome {
            direction,
            undone: false,
            time_to_swipe_ms: time_to_swipe_ms(payload.issued_at, now),
        },
        header_facts: bakeoff.header_facts.clone(),
        provider: bakeoff.provider,
        age_bucket: bakeoff.age_bucket,
        text_tokens_bucket: bakeoff.text_tokens_bucket,
        lang_is_english: bakeoff.lang_is_english,
        input_version: bakeoff.input_version.clone(),
        question_version: bakeoff.question_version.clone(),
        price_version: bakeoff.price_version.clone(),
    }
}

/// Time from the card being served to the swipe, from the `Clock` at swipe time
/// and the token's `issued_at`; never from the client. Negative and oversized
/// values are clamped to `0..=u32::MAX`.
fn time_to_swipe_ms(issued_at: OffsetDateTime, now: OffsetDateTime) -> u32 {
    let ms = (now - issued_at).whole_milliseconds();
    u32::try_from(ms.clamp(0, i128::from(u32::MAX))).unwrap_or(u32::MAX)
}

/// Write the `classifier_eval` record for one swipe (API-SW-1), after the
/// provider action succeeded.
///
/// Returns `None`, and writes nothing, when the direction is `Down` (a skip is
/// not a label, BAKE-6), when the payload has no bake-off part (the user was
/// not consenting when the card was served), or when the user's consent is no
/// longer current (opted out between serve and swipe, CL-02 AC3). A retried
/// swipe (the same `Idempotency-Key`) hits `AlreadyExists`, which is success.
///
/// The write is best effort: a store failure is logged without values and never
/// fails the swipe, so the caller discards the error.
///
/// # Errors
///
/// `Internal` when the store is unavailable. The caller must not fail the swipe
/// on it.
pub async fn write_eval(
    app: &AppState,
    session: &AuthedSession,
    payload: &ClassificationPayload,
    direction: SwipeDirection,
    idempotency_key: &Uuid,
) -> Result<Option<EvalId>, ApiError> {
    let user = session.user;
    // Step 1: a skipped card that comes back is recorded when it is finally
    // swiped, so one card gives at most one labelled record.
    if direction == SwipeDirection::Down {
        return Ok(None);
    }
    // Step 2: no bake-off part means no consent when the card was served.
    let Some(bakeoff) = payload.bakeoff.as_ref() else {
        return Ok(None);
    };
    // Step 3: consent is read now, so an opt-out between serve and swipe writes
    // nothing even when a stale token still holds predictions.
    let Some(stored_user) = load_user(app, &user).await else {
        return Ok(None);
    };
    if !consent_is_current(&stored_user) {
        return Ok(None);
    }
    // Steps 4 to 6.
    let eval_id = eval_id_for(&user, idempotency_key);
    let record = build_eval_record(
        pseudo_id(app, &user),
        payload,
        bakeoff,
        direction,
        app.ports.clock.now(),
        eval_id,
    );
    // Step 7: `AlreadyExists` is a retried swipe and counts as success.
    match app
        .ports
        .store
        .classifier_eval()
        .put(&record, Precondition::MustNotExist)
        .await
    {
        Ok(_) | Err(StoreError::AlreadyExists) => Ok(Some(eval_id)),
        Err(_) => {
            eval_write_failed();
            Err(ApiError::Internal)
        }
    }
}

/// Mark the record the swipe wrote as undone (API-SW-2, BAKE-6). A missing
/// record (TTL, opt-out) is fine, and the flag only goes one way: the app's
/// undo stack pops, so an undo is never undone.
///
/// The write is best effort, like [`write_eval`]: the caller discards the
/// error.
///
/// # Errors
///
/// `Internal` when the store is unavailable. The caller must not fail the undo
/// on it.
pub async fn mark_undone(app: &AppState, eval_id: &EvalId) -> Result<(), ApiError> {
    let repo = app.ports.store.classifier_eval();
    let Ok(stored) = repo.get(eval_id).await else {
        eval_write_failed();
        return Err(ApiError::Internal);
    };
    let Some(stored) = stored else {
        return Ok(());
    };
    if stored.record.outcome.undone {
        return Ok(());
    }
    let mut record = stored.record;
    record.outcome.undone = true;
    match repo
        .put(&record, Precondition::Matches(stored.version))
        .await
    {
        // A concurrent undo, or a record deleted between the read and the
        // write: nothing to flip.
        Ok(_) | Err(StoreError::PreconditionFailed) => Ok(()),
        Err(_) => {
            eval_write_failed();
            Err(ApiError::Internal)
        }
    }
}

/// The user's record, or `None` when there is none or the store refused the
/// read (logged, without values).
async fn load_user(app: &AppState, user: &UserId) -> Option<ports::UserRecord> {
    let Ok(stored) = app.ports.store.users().get(user).await else {
        eval_write_failed();
        return None;
    };
    stored.map(|s| s.record)
}

/// The current pseudonymous user ID, the only user identifier in the record.
fn pseudo_id(app: &AppState, user: &UserId) -> UserPseudoId {
    let pseudo = Pseudonymiser::new(app.config.rate_key.clone()).pseudo_id(&user.0);
    UserPseudoId(pseudo.as_str().to_owned())
}

/// One `op` line for a failed eval write. No values, and the operation name is
/// the store's own, so the log registries stay closed (S5).
fn eval_write_failed() {
    obs::op_log(&obs::OpLog {
        op: "firestore.commit",
        outcome: "failure",
        status: None,
        latency_ms: None,
    });
}
