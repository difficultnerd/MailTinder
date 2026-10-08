//! Block rules created from authenticated, session-bound swipe prompts.
use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::Response;
use domain::user_state::StoredRule;
use domain::{RuleId, RuleKind, RuleMatch, SenderKey, SortRule};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::error::ApiError;
use crate::http::json::{json_ok, ApiJson};
use crate::sealed::{SealedTokens, TokenError, TokenType};
use crate::services::reject::BlockPromptRef;
use crate::services::swipe::{owned_mailbox, wrapped_key, NS_SWIPE};
use crate::services::user_state_store::UserStateStore;
use crate::session::extract::AuthedSession;
use crate::state::AppState;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateBlockRequest {
    pub kind: RuleKind,
    pub prompt_ref: String,
}

#[derive(Serialize)]
struct MatcherDto {
    sender_address: String,
    list_id: Option<String>,
}

#[derive(Serialize)]
struct RuleDto {
    rule_id: Uuid,
    kind: RuleKind,
    #[serde(rename = "match")]
    matcher: MatcherDto,
    category_id: Option<Uuid>,
    enabled: bool,
    #[serde(with = "time::serde::rfc3339")]
    created_at: OffsetDateTime,
    times_applied: u64,
    yearly_rate: Option<u32>,
}

/// API-RULE-2's block branch. Only a server-issued prompt can choose a sender.
///
/// # Errors
/// Rejects invalid, expired or foreign prompts and propagates state errors.
pub async fn create(
    State(app): State<AppState>,
    session: AuthedSession,
    ApiJson(request): ApiJson<CreateBlockRequest>,
) -> Result<Response, ApiError> {
    if request.kind != RuleKind::BlockPerson {
        return Err(ApiError::InvalidRequest {
            fields: vec!["kind".to_owned()],
        });
    }
    let wrapped = wrapped_key(&app, &session.user).await?;
    let prompt = SealedTokens::new(app.ports.keys.clone(), app.ports.clock.clone())
        .open::<BlockPromptRef>(
            TokenType::PromptRef,
            &session.user,
            &wrapped,
            &session.session_record_id,
            &request.prompt_ref,
        )
        .await
        .map_err(|error| match error {
            TokenError::Unavailable => ApiError::ProviderUnavailable {
                mailbox_id: None,
                retry_after_s: None,
            },
            _ => ApiError::InvalidRequest {
                fields: vec!["prompt_ref".to_owned()],
            },
        })?;
    owned_mailbox(&app, &session.user, prompt.mailbox_id).await?;
    let matcher = RuleMatch {
        sender: SenderKey::from_address(&prompt.sender_key),
        list_id: None,
        feedback_id: None,
    };
    let rule = StoredRule {
        rule: SortRule {
            rule_id: RuleId(Uuid::new_v5(
                &NS_SWIPE,
                format!("block:{}", prompt.swipe_id).as_bytes(),
            )),
            kind: RuleKind::BlockPerson,
            matcher: matcher.clone(),
            category: None,
            enabled: true,
            created_at: app.ports.clock.now(),
            source_swipe: Some(prompt.swipe_id),
        },
        times_applied: 0,
        yearly_rate: None,
    };
    crate::services::rules::create_block(&app, &session, rule).await?;
    let state = UserStateStore::new(Arc::new(app.clone()))
        .load(&session.user)
        .await?
        .state;
    let stored = state
        .rules
        .iter()
        .find(|r| {
            r.rule.enabled && r.rule.kind == RuleKind::BlockPerson && r.rule.matcher == matcher
        })
        .ok_or(ApiError::Internal)?;
    Ok(json_ok(
        StatusCode::CREATED,
        &RuleDto {
            rule_id: stored.rule.rule_id.0,
            kind: stored.rule.kind,
            matcher: MatcherDto {
                sender_address: stored.rule.matcher.sender.as_str().to_owned(),
                list_id: stored.rule.matcher.list_id.clone(),
            },
            category_id: None,
            enabled: stored.rule.enabled,
            created_at: stored.rule.created_at,
            times_applied: stored.times_applied,
            yearly_rate: stored.yearly_rate,
        },
    ))
}
