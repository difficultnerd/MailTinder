//! Admin invites: list, create, re-send and revoke (API-ADM-1 to API-ADM-4),
//! and the shared `issue_invite` (AU-01).
//!
//! The raw invite token exists only inside `issue_invite`: it is hashed for the
//! record and placed in the emailed link, and is never stored, logged or
//! returned. Email addresses are sealed with the system key and found again by
//! their keyed lookup hash.

use axum::extract::{Path, RawQuery, State};
use axum::http::StatusCode;
use axum::response::Response;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use domain::{EmailAddress, InviteEvent, InviteState, InviteStatus, Tunables};
use obs::{Pseudonymiser, SecurityEvent, Sensitive};
use ports::store::aad_fields;
use ports::{
    Ciphertext, InviteId, InviteLink, InviteRecord, PageRequest, Precondition, Sha256Hash,
    StoreCursor, StoreError, SystemAad,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use time::format_description::well_known::Rfc3339;
use time::{Duration, OffsetDateTime};
use url::Url;
use uuid::Uuid;

use crate::auth::email_key::email_lookup_hash;
use crate::error::ApiError;
use crate::http::json::{json_ok, ApiJson};
use crate::http::request_id::RequestId;
use crate::limits::{policies, LimitSubject};
use crate::sealed::{SealedTokens, TokenType};
use crate::session::extract::{AdminSession, AuthedSession, SteppedUpAdmin};
use crate::state::AppState;

/// How long an invite link works `[TUNABLE]` (S2 glossary).
pub const INVITE_TTL: Duration = Duration::days(7);
/// How long a finished invite record is kept before the sweeper purges it (S5).
pub const INVITE_PURGE_AFTER: Duration = Duration::days(30);

/// Default and largest page size for the admin lists.
const DEFAULT_LIMIT: u32 = 20;
const MAX_LIMIT: u32 = 50;
/// A page cursor is good for an hour.
const CURSOR_TTL: Duration = Duration::hours(1);

/// `POST /admin/invites` body.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateInvite {
    pub email_address: String,
}

/// The wire form of an invite.
#[derive(Serialize)]
pub struct InviteDto {
    pub invite_id: Uuid,
    pub email_address: String,
    pub status: &'static str,
    pub created_at: String,
    pub expires_at: String,
    pub last_sent_at: String,
}

/// One page of invites.
#[derive(Serialize)]
pub struct InviteList {
    pub invites: Vec<InviteDto>,
    pub next_cursor: Option<String>,
}

/// What `issue_invite` did.
pub enum IssueResult {
    Created(InviteDto),
    Resent(InviteDto),
}

/// Create a new invite for `email`, or re-send the pending one with a new
/// token, then send the email from the admin's primary mailbox (AU-01 AC1,
/// AC2). The record is written before the send, so a send failure leaves it
/// for the admin to re-send.
///
/// # Errors
///
/// `ApiError::Internal` on a store or key failure; the mail error mapping
/// (`502`/`503`/`409`) when the send fails.
pub async fn issue_invite(
    state: &AppState,
    admin: &AuthedSession,
    email: &EmailAddress,
    request_id: RequestId,
) -> Result<IssueResult, ApiError> {
    let ports = &state.ports;
    let now = ports.clock.now();
    let hash = email_lookup_hash(&state.config.email_lookup_key, email);
    let existing = ports
        .store
        .invites()
        .by_email_lookup(&hash)
        .await?
        .into_iter()
        .find(|v| v.record.status == InviteStatus::Pending);

    let raw = URL_SAFE_NO_PAD.encode(ports.rng.bytes32());
    let token_hash = hash_token(&raw);
    let expires_at = now + INVITE_TTL;
    let purge_at = expires_at + INVITE_PURGE_AFTER;

    let (record, resent) = if let Some(found) = existing {
        let mut record = found.record;
        record.token_hash = token_hash;
        record.last_sent_at = now;
        record.expires_at = expires_at;
        record.purge_at = purge_at;
        ports
            .store
            .invites()
            .put(&record, Precondition::Matches(found.version))
            .await?;
        (record, true)
    } else {
        let invite_id = InviteId(ports.rng.uuid_v4());
        let sealed = ports
            .system_keys
            .seal(
                &SystemAad {
                    scope: invite_id.0.to_string(),
                    field: aad_fields::INVITE_EMAIL,
                },
                email.as_str().as_bytes(),
            )
            .await
            .map_err(|_| ApiError::Internal)?;
        let record = InviteRecord {
            invite_id,
            email_address: Ciphertext(sealed),
            email_lookup: hash,
            token_hash,
            status: InviteStatus::Pending,
            created_at: now,
            last_sent_at: now,
            expires_at,
            purge_at,
        };
        ports
            .store
            .invites()
            .put(&record, Precondition::MustNotExist)
            .await?;
        (record, false)
    };

    send(state, admin, email, &raw).await?;
    drop(raw);

    security_event(
        state,
        "invite_create",
        if resent { "resent" } else { "created" },
        Some(admin),
        request_id,
    );
    let dto = to_dto(&record, email.as_str(), now)?;
    Ok(if resent {
        IssueResult::Resent(dto)
    } else {
        IssueResult::Created(dto)
    })
}

/// `GET /admin/invites?status&cursor&limit` (API-ADM-1).
///
/// # Errors
///
/// The route's `ApiError` (S7 4).
pub async fn list(
    State(state): State<AppState>,
    admin: AdminSession,
    RawQuery(query): RawQuery,
) -> Result<Response, ApiError> {
    let params = ListParams::parse(query.as_deref(), true)?;
    let after = open_cursor(&state, &admin.0, params.cursor.as_deref()).await?;
    let page = state
        .ports
        .store
        .invites()
        .list(
            params.status,
            PageRequest {
                limit: params.limit,
                after,
            },
        )
        .await?;
    let now = state.ports.clock.now();
    let mut invites = Vec::with_capacity(page.items.len());
    for item in &page.items {
        let email = open_email(
            &state,
            &item.record.invite_id.0,
            aad_fields::INVITE_EMAIL,
            &item.record.email_address,
        )
        .await?;
        invites.push(to_dto(&item.record, email.expose(), now)?);
    }
    let next_cursor = seal_cursor(&state, &admin.0, page.next).await?;
    Ok(json_ok(
        StatusCode::OK,
        &InviteList {
            invites,
            next_cursor,
        },
    ))
}

/// `POST /admin/invites` (API-ADM-2).
///
/// # Errors
///
/// The route's `ApiError` (S7 4).
pub async fn create(
    State(state): State<AppState>,
    admin: SteppedUpAdmin,
    request_id: RequestId,
    ApiJson(body): ApiJson<CreateInvite>,
) -> Result<Response, ApiError> {
    state
        .limits
        .check(
            &policies::ADMIN_INVITE_SENDS,
            LimitSubject::User(&admin.0.user),
            request_id,
        )
        .await?;
    let email = EmailAddress::parse(&body.email_address).map_err(|_| ApiError::InvalidRequest {
        fields: vec!["/email_address".to_owned()],
    })?;
    Ok(
        match issue_invite(&state, &admin.0, &email, request_id).await? {
            IssueResult::Created(dto) => json_ok(StatusCode::CREATED, &dto),
            IssueResult::Resent(dto) => json_ok(StatusCode::OK, &dto),
        },
    )
}

/// `POST /admin/invites/{id}/resend` (API-ADM-3).
///
/// # Errors
///
/// The route's `ApiError` (S7 4).
pub async fn resend(
    State(state): State<AppState>,
    admin: SteppedUpAdmin,
    request_id: RequestId,
    Path(invite_id): Path<String>,
) -> Result<Response, ApiError> {
    state
        .limits
        .check(
            &policies::ADMIN_INVITE_SENDS,
            LimitSubject::User(&admin.0.user),
            request_id,
        )
        .await?;
    let found = load_invite(&state, &invite_id).await?;
    if found.record.status != InviteStatus::Pending {
        return Err(ApiError::NotFound);
    }
    let email = open_email(
        &state,
        &found.record.invite_id.0,
        aad_fields::INVITE_EMAIL,
        &found.record.email_address,
    )
    .await?;
    let email = EmailAddress::parse(email.expose()).map_err(|_| ApiError::Internal)?;
    match issue_invite(&state, &admin.0, &email, request_id).await? {
        IssueResult::Created(dto) | IssueResult::Resent(dto) => Ok(json_ok(StatusCode::OK, &dto)),
    }
}

/// `DELETE /admin/invites/{id}` (API-ADM-4).
///
/// # Errors
///
/// The route's `ApiError` (S7 4).
pub async fn revoke(
    State(state): State<AppState>,
    admin: SteppedUpAdmin,
    request_id: RequestId,
    Path(invite_id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let found = load_invite(&state, &invite_id).await?;
    match found.record.status {
        InviteStatus::Revoked => return Ok(StatusCode::NO_CONTENT),
        InviteStatus::Used | InviteStatus::Expired => return Err(ApiError::NotFound),
        InviteStatus::Pending => {}
    }
    let now = state.ports.clock.now();
    let current = InviteState {
        status: found.record.status,
        token_hash: found.record.token_hash.0,
        email_hash: found.record.email_lookup.0,
        last_sent_at: found.record.last_sent_at,
        expires_at: found.record.expires_at,
    };
    let next = current
        .apply(InviteEvent::Revoke, &Tunables::default())
        .map_err(|_| ApiError::NotFound)?;
    let mut record = found.record;
    record.status = next.status;
    record.purge_at = now + INVITE_PURGE_AFTER;
    match state
        .ports
        .store
        .invites()
        .put(&record, Precondition::Matches(found.version))
        .await
    {
        Ok(_) => {}
        // Someone else changed it between the read and the write.
        Err(StoreError::PreconditionFailed) => return Err(ApiError::NotFound),
        Err(e) => return Err(e.into()),
    }
    security_event(
        &state,
        "invite_revoke",
        "revoked",
        Some(&admin.0),
        request_id,
    );
    Ok(StatusCode::NO_CONTENT)
}

/// The query parameters of the admin lists.
pub(crate) struct ListParams {
    pub status: Option<InviteStatus>,
    pub cursor: Option<String>,
    pub limit: u32,
}

impl ListParams {
    /// Parse `status` (only when `with_status`), `cursor` and `limit` (1 to 50,
    /// default 20); anything else is `400 invalid_request`.
    pub(crate) fn parse(query: Option<&str>, with_status: bool) -> Result<Self, ApiError> {
        let mut out = Self {
            status: None,
            cursor: None,
            limit: DEFAULT_LIMIT,
        };
        for (key, value) in url::form_urlencoded::parse(query.unwrap_or_default().as_bytes()) {
            match key.as_ref() {
                "status" if with_status => {
                    out.status = Some(match value.as_ref() {
                        "pending" => InviteStatus::Pending,
                        "used" => InviteStatus::Used,
                        "revoked" => InviteStatus::Revoked,
                        "expired" => InviteStatus::Expired,
                        _ => return Err(invalid("/status")),
                    });
                }
                "cursor" => out.cursor = Some(value.into_owned()),
                "limit" => {
                    out.limit = value
                        .parse::<u32>()
                        .ok()
                        .filter(|n| (1..=MAX_LIMIT).contains(n))
                        .ok_or_else(|| invalid("/limit"))?;
                }
                _ => return Err(invalid("")),
            }
        }
        Ok(out)
    }
}

fn invalid(field: &str) -> ApiError {
    ApiError::InvalidRequest {
        fields: if field.is_empty() {
            Vec::new()
        } else {
            vec![field.to_owned()]
        },
    }
}

/// Open a presented page cursor under the admin's key; any failure is
/// `400 invalid_request`.
pub(crate) async fn open_cursor(
    state: &AppState,
    admin: &AuthedSession,
    cursor: Option<&str>,
) -> Result<Option<StoreCursor>, ApiError> {
    let Some(token) = cursor else {
        return Ok(None);
    };
    let user = state
        .ports
        .store
        .users()
        .get(&admin.user)
        .await?
        .ok_or(ApiError::Unauthenticated)?;
    let opened: String = sealed_tokens(state)
        .open(
            TokenType::Cursor,
            &admin.user,
            &user.record.wrapped_data_key,
            &admin.session_record_id,
            token,
        )
        .await
        .map_err(|e| match e {
            crate::sealed::TokenError::Unavailable => ApiError::Internal,
            _ => invalid(""),
        })?;
    Ok(Some(StoreCursor(opened)))
}

/// Seal the store's next cursor for the wire.
pub(crate) async fn seal_cursor(
    state: &AppState,
    admin: &AuthedSession,
    next: Option<StoreCursor>,
) -> Result<Option<String>, ApiError> {
    let Some(next) = next else {
        return Ok(None);
    };
    let user = state
        .ports
        .store
        .users()
        .get(&admin.user)
        .await?
        .ok_or(ApiError::Unauthenticated)?;
    let token = sealed_tokens(state)
        .seal(
            TokenType::Cursor,
            &admin.user,
            &user.record.wrapped_data_key,
            &admin.session_record_id,
            state.ports.clock.now() + CURSOR_TTL,
            &next.0,
        )
        .await
        .map_err(|_| ApiError::Internal)?;
    Ok(Some(token))
}

fn sealed_tokens(state: &AppState) -> SealedTokens {
    SealedTokens::new(state.ports.keys.clone(), state.ports.clock.clone())
}

/// Open a sealed address under the system key.
pub(crate) async fn open_email(
    state: &AppState,
    scope: &Uuid,
    field: &'static str,
    sealed: &Ciphertext,
) -> Result<Sensitive<String>, ApiError> {
    let bytes = state
        .ports
        .system_keys
        .open(
            &SystemAad {
                scope: scope.to_string(),
                field,
            },
            &sealed.0,
        )
        .await
        .map_err(|_| ApiError::Internal)?;
    String::from_utf8(bytes)
        .map(Sensitive::new)
        .map_err(|_| ApiError::Internal)
}

/// RFC 3339 UTC.
pub(crate) fn rfc3339(at: OffsetDateTime) -> Result<String, ApiError> {
    at.to_offset(time::UtcOffset::UTC)
        .format(&Rfc3339)
        .map_err(|_| ApiError::Internal)
}

/// The security event for an admin action: the action and outcome only.
pub(crate) fn security_event(
    state: &AppState,
    action: &'static str,
    outcome: &'static str,
    admin: Option<&AuthedSession>,
    request_id: RequestId,
) {
    obs::security_event(&SecurityEvent {
        action,
        outcome,
        user: admin.map(|a| Pseudonymiser::new(state.config.rate_key.clone()).pseudo_id(&a.user.0)),
        request_id: Some(request_id.0),
        amr: None,
        provider: None,
        method: None,
    });
}

/// SHA-256 of the 43-character token string, exactly as the redemption path
/// hashes the presented token.
fn hash_token(raw: &str) -> Sha256Hash {
    let digest = Sha256::digest(raw.as_bytes());
    let mut out = [0u8; 32];
    out.copy_from_slice(&digest);
    Sha256Hash(out)
}

/// A stored `pending` invite past its expiry is shown as `expired`.
fn display_status(record: &InviteRecord, now: OffsetDateTime) -> &'static str {
    match record.status {
        InviteStatus::Pending if record.expires_at <= now => "expired",
        InviteStatus::Pending => "pending",
        InviteStatus::Used => "used",
        InviteStatus::Revoked => "revoked",
        InviteStatus::Expired => "expired",
    }
}

fn to_dto(record: &InviteRecord, email: &str, now: OffsetDateTime) -> Result<InviteDto, ApiError> {
    Ok(InviteDto {
        invite_id: record.invite_id.0,
        email_address: email.to_owned(),
        status: display_status(record, now),
        created_at: rfc3339(record.created_at)?,
        expires_at: rfc3339(record.expires_at)?,
        last_sent_at: rfc3339(record.last_sent_at)?,
    })
}

async fn load_invite(
    state: &AppState,
    id: &str,
) -> Result<ports::Versioned<InviteRecord>, ApiError> {
    let id = Uuid::parse_str(id).map_err(|_| ApiError::NotFound)?;
    state
        .ports
        .store
        .invites()
        .get(&InviteId(id))
        .await?
        .ok_or(ApiError::NotFound)
}

/// Send the invite email from the admin's primary mailbox.
async fn send(
    state: &AppState,
    admin: &AuthedSession,
    to: &EmailAddress,
    raw: &str,
) -> Result<(), ApiError> {
    let mailboxes = state.ports.store.mailboxes().by_user(&admin.user).await?;
    let primary = mailboxes
        .iter()
        .find(|m| m.record.is_primary)
        .or_else(|| mailboxes.first())
        .ok_or(ApiError::Internal)?;
    let ctx = state
        .tokens
        .mailbox_ctx(state, &admin.user, &primary.record.mailbox_id)
        .await?;
    let url = Url::parse(&format!("{}/#/invite?t={raw}", state.config.app_origin))
        .map_err(|_| ApiError::Internal)?;
    let link = InviteLink(Sensitive::new(url));
    state.invite_mailer.send_invite(&ctx, to, &link).await?;
    Ok(())
}
