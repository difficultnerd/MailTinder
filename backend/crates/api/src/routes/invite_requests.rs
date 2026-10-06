//! Invite requests: a signed-in stranger asks (API-INV-1); the admin lists,
//! approves or declines (API-ADM-5 to API-ADM-7).

use axum::extract::{Path, RawQuery, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use domain::{EmailAddress, InviteStatus};
use ports::store::aad_fields;
use ports::{
    Ciphertext, InviteRequestId, InviteRequestRecord, InviteRequestStatus, PageRequest,
    Precondition, StoreError, SystemAad,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::auth::email_key::email_lookup_hash;
use crate::error::ApiError;
use crate::http::client_ip::client_ip;
use crate::http::json::{json_ok, ApiJson};
use crate::http::request_id::RequestId;
use crate::limits::{policies, LimitSubject};
use crate::routes::invites::{
    issue_invite, open_cursor, open_email, rfc3339, seal_cursor, security_event, IssueResult,
    ListParams,
};
use crate::session::extract::{AdminSession, PendingInviteSession, SteppedUpAdmin};
use crate::session::pre_auth::open_pre_auth;
use crate::state::AppState;

/// The wire form of an invite request.
#[derive(Serialize)]
pub struct InviteRequestDto {
    pub request_id: Uuid,
    pub email_address: String,
    pub created_at: String,
}

/// One page of invite requests.
#[derive(Serialize)]
pub struct InviteRequestList {
    pub requests: Vec<InviteRequestDto>,
    pub next_cursor: Option<String>,
}

/// `POST /invite-requests` body: empty.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NoBody {}

/// `POST /invite-requests` (API-INV-1). The answer is `202` whether or not a
/// request or invite already exists, so nothing can be enumerated.
///
/// # Errors
///
/// The route's `ApiError` (S7 4).
pub async fn request(
    State(state): State<AppState>,
    session: PendingInviteSession,
    request_id: RequestId,
    headers: HeaderMap,
    ApiJson(_body): ApiJson<NoBody>,
) -> Result<StatusCode, ApiError> {
    let ip = client_ip(&headers, state.config.xff_trusted_hops);
    state
        .limits
        .check(
            &policies::INVITE_REQUEST_IP,
            LimitSubject::Ip(&ip),
            request_id,
        )
        .await?;

    let record = &session.loaded.record.record;
    let pre = record.pre_auth.as_ref().ok_or(ApiError::Unauthenticated)?;
    let plain = open_pre_auth(
        state.ports.system_keys.as_ref(),
        &record.session_record_id,
        pre,
    )
    .await?;
    let pending = plain.pending_email.ok_or(ApiError::Unauthenticated)?;
    let email = EmailAddress::parse(pending.expose()).map_err(|_| ApiError::Unauthenticated)?;

    state
        .limits
        .check(
            &policies::INVITE_REQUEST_EMAIL,
            LimitSubject::Email(&email.lookup_form()),
            request_id,
        )
        .await?;

    let hash = email_lookup_hash(&state.config.email_lookup_key, &email);
    let store = &state.ports.store;
    let has_request = store
        .invite_requests()
        .by_email_lookup(&hash)
        .await?
        .is_some();
    let has_invite = store
        .invites()
        .by_email_lookup(&hash)
        .await?
        .iter()
        .any(|v| v.record.status == InviteStatus::Pending);
    if has_request || has_invite {
        return Ok(StatusCode::ACCEPTED);
    }

    let request_id_new = InviteRequestId(state.ports.rng.uuid_v4());
    let sealed = state
        .ports
        .system_keys
        .seal(
            &SystemAad {
                scope: request_id_new.0.to_string(),
                field: aad_fields::INVITE_REQUEST_EMAIL,
            },
            email.as_str().as_bytes(),
        )
        .await
        .map_err(|_| ApiError::Internal)?;
    let created = InviteRequestRecord {
        request_id: request_id_new,
        email_address: Ciphertext(sealed),
        email_lookup: hash,
        created_at: state.ports.clock.now(),
        status: InviteRequestStatus::Pending,
    };
    match store
        .invite_requests()
        .put(&created, Precondition::MustNotExist)
        .await
    {
        // A concurrent identical request already holds the slot.
        Ok(_) | Err(StoreError::AlreadyExists) => Ok(StatusCode::ACCEPTED),
        Err(e) => Err(e.into()),
    }
}

/// `GET /admin/invite-requests?cursor&limit` (API-ADM-5), oldest first.
///
/// # Errors
///
/// The route's `ApiError` (S7 4).
pub async fn list(
    State(state): State<AppState>,
    admin: AdminSession,
    RawQuery(query): RawQuery,
) -> Result<Response, ApiError> {
    let params = ListParams::parse(query.as_deref(), false)?;
    let after = open_cursor(&state, &admin.0, params.cursor.as_deref()).await?;
    let page = state
        .ports
        .store
        .invite_requests()
        .list(PageRequest {
            limit: params.limit,
            after,
        })
        .await?;
    let mut requests = Vec::with_capacity(page.items.len());
    for item in &page.items {
        let email = open_email(
            &state,
            &item.record.request_id.0,
            aad_fields::INVITE_REQUEST_EMAIL,
            &item.record.email_address,
        )
        .await?;
        requests.push(InviteRequestDto {
            request_id: item.record.request_id.0,
            email_address: email.expose().clone(),
            created_at: rfc3339(item.record.created_at)?,
        });
    }
    let next_cursor = seal_cursor(&state, &admin.0, page.next).await?;
    Ok(json_ok(
        StatusCode::OK,
        &InviteRequestList {
            requests,
            next_cursor,
        },
    ))
}

/// `POST /admin/invite-requests/{id}/approve` (API-ADM-6).
///
/// # Errors
///
/// The route's `ApiError` (S7 4).
pub async fn approve(
    State(state): State<AppState>,
    admin: SteppedUpAdmin,
    request_id: RequestId,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    state
        .limits
        .check(
            &policies::ADMIN_INVITE_SENDS,
            LimitSubject::User(&admin.0.user),
            request_id,
        )
        .await?;
    let found = load_request(&state, &id).await?;
    let email = open_email(
        &state,
        &found.record.request_id.0,
        aad_fields::INVITE_REQUEST_EMAIL,
        &found.record.email_address,
    )
    .await?;
    let email = EmailAddress::parse(email.expose()).map_err(|_| ApiError::Internal)?;
    let (IssueResult::Created(dto) | IssueResult::Resent(dto)) =
        issue_invite(&state, &admin.0, &email, request_id).await?;
    state
        .ports
        .store
        .invite_requests()
        .delete(&found.record.request_id, Precondition::None)
        .await?;
    security_event(
        &state,
        "invite_request_approve",
        "approved",
        Some(&admin.0),
        request_id,
    );
    Ok(json_ok(StatusCode::CREATED, &dto))
}

/// `POST /admin/invite-requests/{id}/decline` (API-ADM-7). The requester is
/// not told.
///
/// # Errors
///
/// The route's `ApiError` (S7 4).
pub async fn decline(
    State(state): State<AppState>,
    admin: SteppedUpAdmin,
    request_id: RequestId,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let found = load_request(&state, &id).await?;
    state
        .ports
        .store
        .invite_requests()
        .delete(&found.record.request_id, Precondition::None)
        .await?;
    security_event(
        &state,
        "invite_request_decline",
        "declined",
        Some(&admin.0),
        request_id,
    );
    Ok(StatusCode::NO_CONTENT)
}

async fn load_request(
    state: &AppState,
    id: &str,
) -> Result<ports::Versioned<InviteRequestRecord>, ApiError> {
    let id = Uuid::parse_str(id).map_err(|_| ApiError::NotFound)?;
    state
        .ports
        .store
        .invite_requests()
        .get(&InviteRequestId(id))
        .await?
        .ok_or(ApiError::NotFound)
}
