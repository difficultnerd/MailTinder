//! The `/__fake` control API for `fake-google` (T-205a).
//!
//! Serves only from this crate's binary/library; never in a production binary.

use std::collections::BTreeSet;

use axum::extract::{Path, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::Engine as _;
use serde::Deserialize;
use serde_json::{json, Value};
use time::OffsetDateTime;

use super::gmail::AppState;
use super::scenario::{ClientReg, NextLogin, TokenScenario};
use super::state::{FailRule, FakeEvent, FakeMailboxKey, FakeState, StoredMessage};
use super::tokens::TokenRecord;

/// Build the control router (mounted under `/__fake`).
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/gmail/mailboxes", post(add_mailbox))
        .route("/tokens", post(issue_token))
        .route("/tokens/expire", post(expire_token))
        .route("/gmail/messages", post(seed_message))
        .route("/gmail/seed-corpus", post(seed_corpus))
        .route("/gmail/messages/{email}/{id}", get(read_labels))
        .route("/gmail/sent/{email}", get(read_sent))
        .route("/fail", post(add_fail))
        .route("/gmail/labels-create-race", post(arm_label_race))
        .route("/events", get(read_events))
        .route("/reset", post(reset))
        .route("/identity/clients", post(register_client))
        .route("/identity/next-login", post(set_next_login))
        .route("/identity/token-scenario", post(set_token_scenario))
        .route("/identity/revoke-all", post(revoke_all))
        .route("/identity/revocations", get(read_revocations))
        .route("/identity/service-token", post(service_token))
}

#[derive(Deserialize)]
struct AddMailboxBody {
    email: String,
}

async fn add_mailbox(
    State(st): State<AppState>,
    Json(body): Json<AddMailboxBody>,
) -> Result<Json<Value>, Json<Value>> {
    if !reserved(&body.email) {
        return Err(Json(json!({ "error": "email must use a reserved domain" })));
    }
    let mut st = st.0.lock().unwrap();
    st.mailbox(&body.email);
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
struct IssueTokenBody {
    email: String,
    #[serde(default)]
    scopes: Vec<String>,
    #[serde(default)]
    ttl_s: u64,
}

async fn issue_token(
    State(st): State<AppState>,
    Json(body): Json<IssueTokenBody>,
) -> Result<Json<Value>, Json<Value>> {
    let mut st = st.0.lock().unwrap();
    if !st.mailboxes.contains_key(&body.email) {
        return Err(Json(json!({ "error": "mailbox not found" })));
    }
    let token = format!("tok-{}", uuid::Uuid::new_v4());
    let ttl_s = i64::try_from(body.ttl_s).unwrap_or(i64::MAX);
    let ttl = time::Duration::seconds(ttl_s);
    st.tokens.insert(
        token.clone(),
        TokenRecord {
            mailbox: FakeMailboxKey(body.email.clone()),
            scopes: body.scopes.into_iter().collect(),
            expires_at: OffsetDateTime::now_utc() + ttl,
        },
    );
    Ok(Json(json!({ "access_token": token })))
}

#[derive(Deserialize)]
struct ExpireTokenBody {
    access_token: String,
}

async fn expire_token(
    State(st): State<AppState>,
    Json(body): Json<ExpireTokenBody>,
) -> Result<Json<Value>, Json<Value>> {
    let mut st = st.0.lock().unwrap();
    if let Some(rec) = st.tokens.get_mut(&body.access_token) {
        rec.expires_at = OffsetDateTime::UNIX_EPOCH;
    }
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
struct SeedMessageBody {
    email: String,
    eml_base64: String,
    #[serde(default)]
    labels: Vec<String>,
    #[serde(default)]
    internal_date: Option<String>,
}

async fn seed_message(
    State(st): State<AppState>,
    Json(body): Json<SeedMessageBody>,
) -> Result<Json<Value>, Json<Value>> {
    let mut st = st.0.lock().unwrap();
    if !st.mailboxes.contains_key(&body.email) {
        return Err(Json(json!({ "error": "mailbox not found" })));
    }
    let raw = base64::engine::general_purpose::STANDARD
        .decode(body.eml_base64.as_bytes())
        .map_err(|_| Json(json!({ "error": "bad base64" })))?;
    let internal_date = body
        .internal_date
        .as_deref()
        .and_then(|s| OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339).ok())
        .unwrap_or_else(OffsetDateTime::now_utc);
    let id = st.next_message_id(&body.email);
    let mb = st.mailboxes.get_mut(&body.email).unwrap();
    mb.messages.insert(
        id.clone(),
        StoredMessage {
            id: id.clone(),
            raw,
            labels: body.labels.into_iter().collect(),
            internal_date,
        },
    );
    Ok(Json(json!({ "id": id })))
}

#[derive(Deserialize)]
struct SeedCorpusBody {
    email: String,
}

async fn seed_corpus(
    State(st): State<AppState>,
    Json(body): Json<SeedCorpusBody>,
) -> Result<Json<Value>, Json<Value>> {
    let mut st = st.0.lock().unwrap();
    if !st.mailboxes.contains_key(&body.email) {
        return Err(Json(json!({ "error": "mailbox not found" })));
    }
    let corpus = testkit::corpus::load().map_err(|e| Json(json!({ "error": e })))?;
    let mut ids = Vec::new();
    for cs in &corpus.cases {
        let id = st.next_message_id(&body.email);
        let mb = st.mailboxes.get_mut(&body.email).unwrap();
        mb.messages.insert(
            id.clone(),
            StoredMessage {
                id: id.clone(),
                raw: cs.eml.clone(),
                labels: BTreeSet::from(["INBOX".to_owned()]),
                internal_date: OffsetDateTime::now_utc(),
            },
        );
        ids.push(id);
    }
    Ok(Json(json!({ "ids": ids })))
}

async fn read_labels(
    State(st): State<AppState>,
    Path((email, id)): Path<(String, String)>,
) -> Result<Json<Value>, Json<Value>> {
    let st = st.0.lock().unwrap();
    let mb = st
        .mailboxes
        .get(&email)
        .ok_or_else(|| Json(json!({ "error": "mailbox not found" })))?;
    let m = mb
        .messages
        .get(&id)
        .ok_or_else(|| Json(json!({ "error": "message not found" })))?;
    Ok(Json(
        json!({ "labelIds": m.labels.iter().cloned().collect::<Vec<_>>() }),
    ))
}

async fn read_sent(
    State(st): State<AppState>,
    Path(email): Path<String>,
) -> Result<Json<Value>, Json<Value>> {
    let st = st.0.lock().unwrap();
    let mb = st
        .mailboxes
        .get(&email)
        .ok_or_else(|| Json(json!({ "error": "mailbox not found" })))?;
    let raw: Vec<String> = mb
        .sent
        .iter()
        .map(|b| base64::engine::general_purpose::STANDARD.encode(b))
        .collect();
    Ok(Json(json!({ "raw_base64": raw })))
}

#[derive(Deserialize)]
struct FailBody {
    method: String,
    path_prefix: String,
    status: u16,
    reason: String,
    #[serde(default)]
    retry_after: Option<String>,
    #[serde(default)]
    times: u32,
}

async fn add_fail(
    State(st): State<AppState>,
    Json(body): Json<FailBody>,
) -> Result<Json<Value>, Json<Value>> {
    let mut st = st.0.lock().unwrap();
    st.fail_rules.push(FailRule {
        method: body.method,
        path_prefix: body.path_prefix,
        status: body.status,
        reason: body.reason,
        retry_after: body.retry_after,
        times: body.times.max(1),
    });
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
struct RaceBody {
    email: String,
}

async fn arm_label_race(
    State(st): State<AppState>,
    Json(body): Json<RaceBody>,
) -> Result<Json<Value>, Json<Value>> {
    let mut st = st.0.lock().unwrap();
    st.label_create_race.insert(body.email);
    Ok(Json(json!({ "ok": true })))
}

async fn read_events(State(st): State<AppState>) -> Result<Json<Value>, Json<Value>> {
    let st = st.0.lock().unwrap();
    let events: Vec<Value> = st
        .events
        .iter()
        .map(|e| match e {
            FakeEvent::Request {
                method,
                route,
                query,
            } => json!({
                "type": "request",
                "method": method,
                "route": route,
                "query": query,
            }),
            FakeEvent::PermanentDeleteAttempted { method, path } => json!({
                "type": "permanent_delete_attempted",
                "method": method,
                "path": path,
            }),
        })
        .collect();
    Ok(Json(json!({ "events": events })))
}

async fn reset(State(st): State<AppState>) -> Result<Json<Value>, Json<Value>> {
    let mut st = st.0.lock().unwrap();
    *st = FakeState::default();
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
struct ClientBody {
    client_id: String,
    client_secret: String,
    redirect_uris: Vec<String>,
}

async fn register_client(
    State(st): State<AppState>,
    Json(body): Json<ClientBody>,
) -> Result<Json<Value>, Json<Value>> {
    let mut st = st.0.lock().unwrap();
    st.clients.insert(
        body.client_id.clone(),
        ClientReg {
            client_id: body.client_id,
            client_secret: body.client_secret,
            redirect_uris: body.redirect_uris,
        },
    );
    Ok(Json(json!({ "ok": true })))
}

async fn set_next_login(
    State(st): State<AppState>,
    Json(body): Json<NextLogin>,
) -> Result<Json<Value>, Json<Value>> {
    let mut st = st.0.lock().unwrap();
    st.next_login = Some(body);
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
struct ScenarioBody {
    scenario: TokenScenario,
}

async fn set_token_scenario(
    State(st): State<AppState>,
    Json(body): Json<ScenarioBody>,
) -> Result<Json<Value>, Json<Value>> {
    let mut st = st.0.lock().unwrap();
    st.token_scenario = Some(body.scenario);
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
struct RevokeAllBody {
    sub: String,
}

async fn revoke_all(
    State(st): State<AppState>,
    Json(body): Json<RevokeAllBody>,
) -> Result<Json<Value>, Json<Value>> {
    let mut st = st.0.lock().unwrap();
    let subs: Vec<String> = st
        .refresh_tokens
        .iter()
        .filter(|(_, r)| r.sub == body.sub)
        .map(|(t, _)| t.clone())
        .collect();
    for t in subs {
        if let Some(rec) = st.refresh_tokens.remove(&t) {
            st.revocations.push("refresh".to_owned());
            let to_remove: Vec<String> = st
                .grants
                .get(&rec.grant)
                .map(|g| g.access_tokens.clone())
                .unwrap_or_default();
            for at in &to_remove {
                st.tokens.remove(at);
            }
        }
    }
    Ok(Json(json!({ "ok": true })))
}

async fn read_revocations(State(st): State<AppState>) -> Result<Json<Value>, Json<Value>> {
    let st = st.0.lock().unwrap();
    Ok(Json(json!({ "revocations": st.revocations })))
}

#[derive(Deserialize)]
struct ServiceTokenBody {
    aud: String,
    email: String,
    ttl_s: u64,
}

async fn service_token(
    State(st): State<AppState>,
    Json(body): Json<ServiceTokenBody>,
) -> Result<Json<Value>, Json<Value>> {
    let ttl_s = i64::try_from(body.ttl_s).unwrap_or(i64::MAX);
    let ttl = time::Duration::seconds(ttl_s);
    let token = super::oidc::build_id_token(
        &super::oidc::IdTokenOptions {
            aud: Some(&body.aud),
            sub: "service-sub",
            email: &body.email,
            email_verified: true,
            now: OffsetDateTime::now_utc().unix_timestamp(),
            exp_offset: ttl.whole_seconds(),
            ..Default::default()
        },
        None,
    );
    let _ = st;
    Ok(Json(json!({ "id_token": token })))
}

#[allow(clippy::case_sensitive_file_extension_comparisons)]
fn reserved(email: &str) -> bool {
    let domain = email.rsplit('@').next().unwrap_or("");
    let d = domain.to_lowercase();
    d == "example.com"
        || d == "example.net"
        || d == "example.org"
        || d.ends_with(".test")
        || d.ends_with(".invalid")
        || d.ends_with(".example.com")
        || d.ends_with(".example.net")
        || d.ends_with(".example.org")
}
