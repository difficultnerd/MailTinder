//! API-RULE-1 to API-RULE-5: the Rules screen and the block and keep prompts
//! (T-608; SR-01 AC4, PB-01 AC2/AC3, FL-04 AC2, ST-01 AC2, GM-05 AC2).
//!
//! Five handlers: list rules (optionally by kind), create a `block_person`
//! rule from a server-sealed prompt reference or a `file` rule from a message
//! re-read from the provider, switch a rule on or off, delete it, and decline a
//! block prompt for the sender for 90 days.
//!
//! The matchers are always built server-side: the `block_person` sender comes
//! from the sealed prompt, the `file` sender from a fresh provider read, never
//! from the request. No sender address, List-Id or message ID is logged (S5,
//! C2).

use axum::extract::{Path, RawQuery, State};
use axum::http::StatusCode;
use axum::response::Response;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::error::ApiError;
use crate::http::json::{json_ok, ApiJson};
use crate::http::request_id::RequestId;
use crate::session::extract::AuthedSession;
use crate::state::AppState;

/// `[DEFAULT]` fixed namespace for the deterministic `block_person` rule ID
/// (API-RULE-2): `v5(NS_RULE_FROM_PROMPT, user ++ swipe_id)`, so a retry of the
/// same prompt finds the same rule.
pub const NS_RULE_FROM_PROMPT: Uuid = Uuid::from_u128(0x6d74_7275_0000_4000_8000_0000_0000_0001);

/// One sort rule (schema `Rule`).
#[derive(Serialize)]
pub struct RuleDto {
    /// The rule ID.
    pub rule_id: Uuid,
    /// One of `reject_list`, `block_person`, `file`.
    pub kind: &'static str,
    /// What the rule matches.
    pub r#match: RuleMatchDto,
    /// The filing category, for a `file` rule; otherwise `null`.
    pub category_id: Option<Uuid>,
    /// Whether the rule still matches mail (SR-01 AC4).
    pub enabled: bool,
    /// When the rule was created (RFC 3339).
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    /// How many messages the rule has acted on.
    pub times_applied: u64,
    /// The yearly estimate of stopped mail, or `null` when unknown (GM-05 AC2).
    pub yearly_rate: Option<u32>,
}

/// The matcher of a rule (schema `Rule` `match`). The Feedback-ID key stays
/// internal.
#[derive(Serialize)]
pub struct RuleMatchDto {
    /// The normalised sender address the rule matches.
    pub sender_address: String,
    /// The List-Id the rule matches, or `null`.
    pub list_id: Option<String>,
}

/// The `GET /rules` response body.
#[derive(Serialize)]
pub struct RuleListDto {
    /// The user's rules, newest first.
    pub rules: Vec<RuleDto>,
}

/// The request body of API-RULE-2.
///
/// A flat shape, not an internally tagged enum: `serde` cannot reliably combine
/// an internal tag with `deny_unknown_fields`, and every unknown field must
/// give `400`. [`CreateRuleDto::validate`] enforces the two documented shapes by
/// hand.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateRuleDto {
    /// `block_person` or `file`; `reject_list` is refused (API-SW-1 only).
    pub kind: String,
    /// The sealed block prompt reference, for `block_person`.
    #[serde(default)]
    pub prompt_ref: Option<String>,
    /// The mailbox the message is in, for `file`.
    #[serde(default)]
    pub mailbox_id: Option<Uuid>,
    /// The message the filing rule is built from, for `file`.
    #[serde(default)]
    pub message_id: Option<String>,
    /// The category the filing rule files into, for `file`.
    #[serde(default)]
    pub category_id: Option<Uuid>,
}

/// The validated two shapes of API-RULE-2.
pub enum CreateRule {
    /// A `block_person` rule from a sealed prompt reference.
    BlockPerson {
        /// The opaque sealed prompt reference.
        prompt_ref: String,
    },
    /// A `file` rule from a message.
    File {
        /// The mailbox that holds the message.
        mailbox_id: Uuid,
        /// The message to read the sender from.
        message_id: String,
        /// The category the rule files into.
        category_id: Uuid,
    },
}

impl CreateRuleDto {
    /// Validate the flat body into one of the two shapes. Any unknown field,
    /// missing field, `reject_list`, or a field belonging to the other shape is
    /// `400 invalid_request` (S7 5.7).
    ///
    /// # Errors
    ///
    /// `ApiError::InvalidRequest` for every malformed body.
    pub fn validate(self) -> Result<CreateRule, ApiError> {
        match self.kind.as_str() {
            "block_person" => {
                let prompt_ref = self.prompt_ref.ok_or_else(|| invalid("prompt_ref"))?;
                if self.mailbox_id.is_some()
                    || self.message_id.is_some()
                    || self.category_id.is_some()
                {
                    return Err(invalid("kind"));
                }
                Ok(CreateRule::BlockPerson { prompt_ref })
            }
            "file" => {
                let (Some(mailbox_id), Some(message_id), Some(category_id)) =
                    (self.mailbox_id, self.message_id, self.category_id)
                else {
                    return Err(invalid("kind"));
                };
                if self.prompt_ref.is_some() {
                    return Err(invalid("kind"));
                }
                Ok(CreateRule::File {
                    mailbox_id,
                    message_id,
                    category_id,
                })
            }
            // `reject_list` is created only by API-SW-1, and any other kind is
            // unknown (S7 5.7).
            _ => Err(invalid("kind")),
        }
    }
}

/// The request body of API-RULE-3.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PatchRuleDto {
    /// Whether the rule should match mail.
    pub enabled: bool,
}

/// The request body of API-RULE-5.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeclineDto {
    /// The opaque sealed prompt reference.
    pub prompt_ref: String,
}

/// `GET /api/v1/rules?kind=` (API-RULE-1).
///
/// # Errors
///
/// `400 invalid_request` for an unknown `kind` or query parameter; the state
/// file errors of the user state store.
pub async fn list(
    State(app): State<AppState>,
    session: AuthedSession,
    RawQuery(query): RawQuery,
) -> Result<Response, ApiError> {
    let kind = kind_param(query.as_deref())?;
    let rules = crate::services::rules::list(&app, &session, kind).await?;
    Ok(json_ok(StatusCode::OK, &RuleListDto { rules }))
}

/// `POST /api/v1/rules` (API-RULE-2).
///
/// # Errors
///
/// `400 invalid_request` for a malformed body or a sealed reference that does
/// not open; `404 not_found` for a mailbox that is not the user's or an unknown
/// category; `409 message_changed` when the message is gone; provider and state
/// file errors.
pub async fn create(
    State(app): State<AppState>,
    session: AuthedSession,
    request_id: RequestId,
    ApiJson(body): ApiJson<CreateRuleDto>,
) -> Result<Response, ApiError> {
    let rule = match body.validate()? {
        CreateRule::BlockPerson { prompt_ref } => {
            crate::services::rules::create_block(&app, &session, &prompt_ref).await?
        }
        CreateRule::File {
            mailbox_id,
            message_id,
            category_id,
        } => {
            crate::services::rules::create_file(
                &app,
                &session,
                mailbox_id,
                &message_id,
                category_id,
                Some(request_id.0),
            )
            .await?
        }
    };
    Ok(json_ok(StatusCode::CREATED, &rule))
}

/// `PATCH /api/v1/rules/{rule_id}` (API-RULE-3).
///
/// # Errors
///
/// `404 not_found` for a missing or malformed ID, and the state file errors of
/// the user state store.
pub async fn patch(
    State(app): State<AppState>,
    session: AuthedSession,
    Path(raw): Path<String>,
    request_id: RequestId,
    ApiJson(body): ApiJson<PatchRuleDto>,
) -> Result<Response, ApiError> {
    let id = rule_id(&raw)?;
    let rule =
        crate::services::rules::patch(&app, &session, id, body.enabled, Some(request_id.0)).await?;
    Ok(json_ok(StatusCode::OK, &rule))
}

/// `DELETE /api/v1/rules/{rule_id}` (API-RULE-4).
///
/// # Errors
///
/// `404 not_found` for a missing or malformed ID, and the state file errors of
/// the user state store.
pub async fn delete(
    State(app): State<AppState>,
    session: AuthedSession,
    Path(raw): Path<String>,
    request_id: RequestId,
) -> Result<StatusCode, ApiError> {
    let id = rule_id(&raw)?;
    crate::services::rules::delete(&app, &session, id, Some(request_id.0)).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /api/v1/block-prompts/decline` (API-RULE-5).
///
/// # Errors
///
/// `400 invalid_request` for a sealed reference that does not open, and the
/// state file errors of the user state store.
pub async fn decline(
    State(app): State<AppState>,
    session: AuthedSession,
    ApiJson(body): ApiJson<DeclineDto>,
) -> Result<StatusCode, ApiError> {
    crate::services::rules::decline(&app, &session, &body.prompt_ref).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Parse the optional `kind` filter (API-RULE-1): one of the three kinds, or
/// `400` for anything else or an unknown query parameter.
fn kind_param(query: Option<&str>) -> Result<Option<&'static str>, ApiError> {
    let mut kind = None;
    for (key, value) in url::form_urlencoded::parse(query.unwrap_or_default().as_bytes()) {
        match key.as_ref() {
            "kind" => {
                kind = Some(match value.as_ref() {
                    "reject_list" => "reject_list",
                    "block_person" => "block_person",
                    "file" => "file",
                    _ => return Err(invalid("kind")),
                });
            }
            _ => return Err(invalid("")),
        }
    }
    Ok(kind)
}

/// A malformed path UUID is a missing rule: no parse detail leaks (S7 4).
fn rule_id(raw: &str) -> Result<Uuid, ApiError> {
    Uuid::parse_str(raw).map_err(|_| ApiError::NotFound)
}

/// A `400 invalid_request` naming the offending field.
fn invalid(field: &str) -> ApiError {
    ApiError::InvalidRequest {
        fields: if field.is_empty() {
            Vec::new()
        } else {
            vec![field.to_owned()]
        },
    }
}
