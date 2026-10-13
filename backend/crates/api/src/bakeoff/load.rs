//! Records to report rows (T-908b; S7 5.13 API-ADM-10).
//!
//! `load_rows` pages the range's `classifier_eval` records and converts each one
//! into the domain's [`EvalRow`]. The one derived value is the participant
//! index: it is assigned in order of first appearance of each
//! `user_pseudo_id`, so the report can cluster by participant and count distinct
//! contributors without ever holding a pseudonymous ID. That map lives only for
//! the call and is dropped before it returns (S10 9.4, EXP-3); nothing here
//! logs, and no field of a row carries a user ID, an eval ID or message text.

use std::collections::HashMap;

use domain::bakeoff::labels::SwipeDirection as DomainDirection;
use domain::bakeoff::report::{EvalRow, Prediction, PredictionError, ReportQuery, Segments};
use domain::Provider;
use ports::store::{ClassifierEvalRecord, MAX_PAGE};
use ports::{
    AgeBucket, ModelErrorCode, ModelPrediction, PageRequest, StoreCursor, SwipeDirection,
    TextTokensBucket,
};
use time::Duration;

use crate::error::ApiError;
use crate::state::AppState;

/// The eight `EvalHeaderFacts` names the report segments by (CR-01a G4, S7 5.13).
const FACT_NAMES: [&str; 8] = [
    "has_list_unsubscribe",
    "dkim_covers_list_unsubscribe",
    "has_list_unsubscribe_post",
    "has_list_id",
    "has_feedback_id",
    "precedence_bulk",
    "auto_submitted",
    "from_authenticated",
];

/// Every row the query's range covers, oldest first (the store's own order).
///
/// The range is `from` at 00:00 UTC, inclusive, to `to` plus one day, exclusive,
/// so the last day's whole calendar range is in (S7 5.13: the parameters are
/// dates).
///
/// # Errors
///
/// `ApiError::Internal` when a store page fails; the participant map is dropped
/// here, never returned.
pub async fn load_rows(app: &AppState, q: &ReportQuery) -> Result<Vec<EvalRow>, ApiError> {
    let from = q.from.midnight().assume_utc();
    let to = q.to.midnight().assume_utc() + Duration::days(1);
    let mut rows: Vec<EvalRow> = Vec::new();
    // Participant indexes in order of first appearance; dropped on return.
    let mut participants: HashMap<String, u32> = HashMap::new();
    let mut after: Option<StoreCursor> = None;
    loop {
        let page = app
            .ports
            .store
            .classifier_eval()
            .range(
                from,
                to,
                PageRequest {
                    limit: MAX_PAGE,
                    after: after.clone(),
                },
            )
            .await?;
        for record in &page.items {
            rows.push(row_of(record, &mut participants));
        }
        match page.next {
            Some(next) if !page.items.is_empty() => after = Some(next),
            _ => return Ok(rows),
        }
    }
}

/// One record as a report row, assigning `participants` an index if the record's
/// pseudonymous ID has not been seen yet.
fn row_of(record: &ClassifierEvalRecord, participants: &mut HashMap<String, u32>) -> EvalRow {
    let next = u32::try_from(participants.len()).unwrap_or(u32::MAX);
    let participant = *participants
        .entry(record.user_pseudo_id.0.clone())
        .or_insert(next);
    EvalRow {
        participant,
        created_at: record.created_at,
        header_rules: (record.header_rules.class, record.header_rules.score),
        gemini: record.gemini.as_ref().map(prediction),
        jev: record.jev.as_ref().map(prediction),
        direction: direction(record.outcome.direction),
        undone: record.outcome.undone,
        segments: Segments {
            facts: [
                (FACT_NAMES[0], record.header_facts.has_list_unsubscribe),
                (
                    FACT_NAMES[1],
                    record.header_facts.dkim_covers_list_unsubscribe,
                ),
                (FACT_NAMES[2], record.header_facts.has_list_unsubscribe_post),
                (FACT_NAMES[3], record.header_facts.has_list_id),
                (FACT_NAMES[4], record.header_facts.has_feedback_id),
                (FACT_NAMES[5], record.header_facts.precedence_bulk),
                (FACT_NAMES[6], record.header_facts.auto_submitted),
                (FACT_NAMES[7], record.header_facts.from_authenticated),
            ],
            provider: provider_str(record.provider),
            age_bucket: age_bucket_str(record.age_bucket),
            text_tokens_bucket: text_tokens_bucket_str(record.text_tokens_bucket),
            lang_is_english: record.lang_is_english,
        },
        input_version: record.input_version.clone(),
        question_version: record.question_version.clone(),
        price_version: record.price_version.clone(),
    }
}

/// One stored prediction as the domain's mirror of it.
fn prediction(stored: &ModelPrediction) -> Prediction {
    Prediction {
        class: stored.class,
        score: stored.score,
        // The record's confidence is `f32`; the report computes in `f64`.
        confidence: stored.confidence.map(f64::from),
        latency_ms: stored.latency_ms,
        input_tokens: stored.input_tokens,
        error: stored.error_code.map(error),
        classifier_id: stored.model_version.clone(),
    }
}

/// A stored model error as the domain's two coarse cases: the report only
/// separates the timeout from everything else (S7 5.13 `timeout_rate`).
fn error(code: ModelErrorCode) -> PredictionError {
    match code {
        ModelErrorCode::Timeout => PredictionError::Timeout,
        ModelErrorCode::Http
        | ModelErrorCode::InvalidOutput
        | ModelErrorCode::Disabled
        | ModelErrorCode::Unavailable => PredictionError::Other,
    }
}

/// The report's own swipe direction for the store's.
fn direction(store: SwipeDirection) -> DomainDirection {
    match store {
        SwipeDirection::Left => DomainDirection::Left,
        SwipeDirection::Right => DomainDirection::Right,
        SwipeDirection::Up => DomainDirection::Up,
        SwipeDirection::Down => DomainDirection::Down,
    }
}

/// The segment value of a provider (CR-01a G4). Exhaustive: a v2 provider has to
/// be reviewed here (XC-02).
fn provider_str(provider: Provider) -> &'static str {
    match provider {
        Provider::Gmail => "gmail",
    }
}

/// The segment value of an age bucket (the stored wire names).
fn age_bucket_str(bucket: AgeBucket) -> &'static str {
    match bucket {
        AgeBucket::Under7d => "<7d",
        AgeBucket::D7To90d => "7d-90d",
        AgeBucket::D90To1y => "90d-1y",
        AgeBucket::Y1To5y => "1y-5y",
        AgeBucket::Over5y => ">5y",
    }
}

/// The segment value of a text length bucket (the stored wire names).
fn text_tokens_bucket_str(bucket: TextTokensBucket) -> &'static str {
    match bucket {
        TextTokensBucket::Under100 => "<100",
        TextTokensBucket::T100To300 => "100-300",
        TextTokensBucket::Over300 => ">300",
    }
}
