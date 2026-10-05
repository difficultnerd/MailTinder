//! Gmail REST routes for `fake-google` (T-205a).
//!
//! Route table (base path `/gmail/v1/users/me`; `{userId}` other than `me`
//! returns 400):
//!
//! | Method and path | Behaviour |
//! | --- | --- |
//! | `GET /messages` | `labelIds` (repeatable, AND), `q`, `pageToken`, `maxResults` (default 100, max 500), `includeSpamTrash` (default false). Newest first by `internalDate`, ties by ID. |
//! | `GET /messages/{id}` | `format` = `metadata` (only named headers, in order, duplicates kept), `full` (MIME tree), `minimal` (no payload). |
//! | `POST /messages/{id}/modify` | add/remove labels. |
//! | `POST /messages/{id}/trash` | adds `TRASH`, removes `INBOX`. |
//! | `POST /messages/{id}/untrash` | removes `TRASH`. |
//! | `POST /messages/send` | stores a new `SENT` message. |
//! | `GET /labels`, `GET /labels/{id}`, `POST /labels` | label listing and creation. |
//! | `GET /profile` | profile summary. |
//! | `DELETE /messages/{id}`, `POST /messages/batchDelete`, `DELETE /threads/{id}` | 500 + `PermanentDeleteAttempted` (INV-5). |

use std::collections::BTreeSet;
use std::sync::Arc;

use axum::extract::{Path, RawQuery, State};
use axum::http::HeaderMap;
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use time::OffsetDateTime;

use super::errors::GmailError;
use super::mime;
use super::state::{FakeEvent, FakeState, StoredMessage};
use super::tokens::GMAIL_MODIFY;

/// The shared state wrapper used by axum. `.0` is the shared fake state;
/// `.1` is the injected clock (all times come from it).
#[derive(Clone)]
pub struct AppState(
    pub Arc<std::sync::Mutex<FakeState>>,
    pub Arc<dyn ports::Clock>,
);

impl AppState {
    /// The current time from the injected clock.
    pub fn now(&self) -> OffsetDateTime {
        self.1.now()
    }
}

/// Build the Gmail router (mounted under `/gmail/v1/users/me`).
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/messages", get(list_messages))
        .route("/messages/send", post(send_message))
        .route("/messages/batchDelete", post(batch_delete))
        .route("/messages/{id}", get(get_message))
        .route("/messages/{id}/modify", post(modify_message))
        .route("/messages/{id}/trash", post(trash_message))
        .route("/messages/{id}/untrash", post(untrash_message))
        .route("/labels", get(list_labels).post(create_label))
        .route("/labels/{id}", get(get_label))
        .route("/profile", get(get_profile))
        .route("/threads/{id}", delete(delete_thread))
        .route("/messages/{id}", delete(delete_message))
}

/// The authenticated mailbox email, resolved from the bearer token.
fn authed_email(state: &FakeState, headers: &HeaderMap) -> Result<String, GmailError> {
    let auth = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| GmailError::new(401, "authError"))?;
    let token = auth
        .strip_prefix("Bearer ")
        .ok_or_else(|| GmailError::new(401, "authError"))?;
    let rec = state
        .tokens
        .get(token)
        .ok_or_else(|| GmailError::new(401, "authError"))?;
    if rec.expires_at <= OffsetDateTime::now_utc() {
        return Err(GmailError::new(401, "authError"));
    }
    Ok(rec.mailbox.0.clone())
}

/// Check the token carries a scope.
fn require_scope(
    state: &FakeState,
    headers: &HeaderMap,
    scope: &str,
) -> Result<String, GmailError> {
    let auth = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| GmailError::new(401, "authError"))?;
    let token = auth
        .strip_prefix("Bearer ")
        .ok_or_else(|| GmailError::new(401, "authError"))?;
    let rec = state
        .tokens
        .get(token)
        .ok_or_else(|| GmailError::new(401, "authError"))?;
    if rec.expires_at <= OffsetDateTime::now_utc() {
        return Err(GmailError::new(401, "authError"));
    }
    if !rec.scopes.contains(scope) {
        return Err(GmailError::new(403, "insufficientPermissions"));
    }
    Ok(rec.mailbox.0.clone())
}

/// Apply the first matching failure rule; returns `Some(error)` if one fired.
pub(crate) fn check_fail(state: &mut FakeState, method: &str, path: &str) -> Option<GmailError> {
    let idx = state
        .fail_rules
        .iter()
        .position(|r| r.method == method && path.starts_with(&r.path_prefix) && r.times > 0)?;
    let rule = &mut state.fail_rules[idx];
    rule.times -= 1;
    let mut err = GmailError::new(rule.status, rule.reason.clone());
    if let Some(ra) = &rule.retry_after {
        err = err.with_retry_after(ra.clone());
    }
    Some(err)
}

pub(crate) fn record(
    state: &mut FakeState,
    method: &str,
    route: &str,
    query: Vec<(String, String)>,
) {
    state.events.push(FakeEvent::Request {
        method: method.to_owned(),
        route: route.to_owned(),
        query,
    });
}

fn record_delete(state: &mut FakeState, method: &str, path: &str) {
    state.events.push(FakeEvent::PermanentDeleteAttempted {
        method: method.to_owned(),
        path: path.to_owned(),
    });
}

#[derive(Default, Serialize)]
#[allow(non_snake_case)]
struct ListQuery {
    #[serde(default)]
    labelIds: Vec<String>,
    #[serde(default)]
    q: Option<String>,
    #[serde(default)]
    pageToken: Option<String>,
    #[serde(default)]
    maxResults: Option<u32>,
    #[serde(default)]
    includeSpamTrash: Option<bool>,
}

/// Parse the raw list query string: repeated `labelIds` allowed, unknown
/// parameters ignored (Gmail tolerates `fields=` and friends).
fn parse_list_query(raw: &str) -> ListQuery {
    let mut q = ListQuery::default();
    for (k, v) in url::form_urlencoded::parse(raw.as_bytes()) {
        match k.as_ref() {
            "labelIds" => q.labelIds.push(v.into_owned()),
            "q" => q.q = Some(v.into_owned()),
            "pageToken" => q.pageToken = Some(v.into_owned()),
            "maxResults" => q.maxResults = v.parse().ok(),
            "includeSpamTrash" => q.includeSpamTrash = v.parse().ok(),
            _ => {}
        }
    }
    q
}

async fn list_messages(
    State(st): State<AppState>,
    headers: HeaderMap,
    RawQuery(raw): RawQuery,
) -> Result<Json<Value>, GmailError> {
    let q = parse_list_query(raw.as_deref().unwrap_or(""));
    let mut st = st.0.lock().unwrap();
    let email = require_scope(&st, &headers, GMAIL_MODIFY)?;
    if let Some(e) = check_fail(&mut st, "GET", "/gmail/v1/users/me/messages") {
        return Err(e);
    }
    record(&mut st, "GET", "messages.list", query_pairs(&q));
    if let Some(qs) = &q.q {
        if !q_terms_valid(qs) {
            return Err(GmailError::new(400, "invalidArgument"));
        }
    }
    let mb = st
        .mailboxes
        .get(&email)
        .ok_or_else(|| GmailError::new(404, "notFound"))?;

    let max = q.maxResults.unwrap_or(100).min(500);
    let include_trash = q.includeSpamTrash.unwrap_or(false);

    // Filter by labels (AND) and q.
    let mut ids: Vec<&StoredMessage> = mb.messages.values().collect();
    ids.retain(|m| {
        let labels_ok = q.labelIds.iter().all(|l| m.labels.contains(l));
        let trash_ok = include_trash || (!m.labels.contains("TRASH") && !m.labels.contains("SPAM"));
        let q_ok = q_terms_match(q.q.as_deref(), m);
        labels_ok && trash_ok && q_ok
    });
    // Newest first by internalDate, ties by ID.
    ids.sort_by(|a, b| {
        b.internal_date
            .cmp(&a.internal_date)
            .then_with(|| b.id.cmp(&a.id))
    });

    // Paging.
    let offset = match &q.pageToken {
        Some(t) => t
            .strip_prefix('p')
            .and_then(|n| n.parse::<usize>().ok())
            .ok_or_else(|| GmailError::new(400, "invalidArgument"))?,
        None => 0,
    };
    let page: Vec<&StoredMessage> = ids
        .iter()
        .skip(offset)
        .take(max as usize)
        .copied()
        .collect();
    let next = if offset + page.len() < ids.len() {
        Some(format!("p{}", offset + page.len()))
    } else {
        None
    };

    let messages: Vec<Value> = page
        .iter()
        .map(|m| json!({ "id": m.id, "threadId": m.id }))
        .collect();
    let mut body = json!({ "resultSizeEstimate": ids.len() });
    if !messages.is_empty() {
        body["messages"] = json!(messages);
    }
    if let Some(n) = next {
        body["nextPageToken"] = json!(n);
    }
    Ok(Json(body))
}

fn query_pairs<T: serde::Serialize>(q: &T) -> Vec<(String, String)> {
    let Ok(Value::Object(map)) = serde_json::to_value(q) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (k, v) in map {
        match v {
            Value::String(s) => out.push((k, s)),
            Value::Array(a) => {
                for x in a {
                    if let Value::String(s) = x {
                        out.push((k.clone(), s));
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// True if every `q` term is one of the supported subset.
fn q_terms_valid(q: &str) -> bool {
    q.split_whitespace().all(|t| {
        t.starts_with("after:")
            || t.starts_with("before:")
            || t.starts_with("from:")
            || t.starts_with("list:")
    })
}

/// Match the `q` subset: `after:<secs>`, `before:<secs>`, `from:<addr>`, `list:<id>`.
fn q_terms_match(q: Option<&str>, m: &StoredMessage) -> bool {
    let Some(q) = q else { return true };
    let terms: Vec<&str> = q.split_whitespace().collect();
    for t in terms {
        if let Some(secs) = t.strip_prefix("after:") {
            let ok = match secs.parse::<i64>() {
                Ok(b) => m.internal_date.unix_timestamp() > b,
                Err(_) => false,
            };
            if !ok {
                return false;
            }
        } else if let Some(secs) = t.strip_prefix("before:") {
            let ok = match secs.parse::<i64>() {
                Ok(b) => m.internal_date.unix_timestamp() < b,
                Err(_) => false,
            };
            if !ok {
                return false;
            }
        } else if let Some(addr) = t.strip_prefix("from:") {
            let raw = String::from_utf8_lossy(&m.raw);
            if !raw
                .to_lowercase()
                .contains(&format!("from: {}", addr.to_lowercase()))
            {
                return false;
            }
        } else if let Some(list) = t.strip_prefix("list:") {
            let raw = String::from_utf8_lossy(&m.raw);
            if !raw
                .to_lowercase()
                .contains(&format!("list-id: {}", list.to_lowercase()))
            {
                return false;
            }
        } else {
            // Unknown term: the adapter should not send it; treat as no match.
            return false;
        }
    }
    true
}

#[derive(Default)]
struct GetQuery {
    format: Option<String>,
    metadata_headers: Vec<String>,
}

impl GetQuery {
    fn query_pairs(&self) -> Vec<(String, String)> {
        let mut out = Vec::new();
        if let Some(f) = &self.format {
            out.push(("format".to_owned(), f.clone()));
        }
        for h in &self.metadata_headers {
            out.push(("metadataHeaders".to_owned(), h.clone()));
        }
        out
    }
}

/// Parse the raw query string, collecting repeated `metadataHeaders` values.
fn parse_get_query(raw: &str) -> GetQuery {
    let mut q = GetQuery::default();
    for pair in raw.split('&').filter(|p| !p.is_empty()) {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        match k {
            "format" => q.format = Some(v.to_owned()),
            "metadataHeaders" => q.metadata_headers.push(v.to_owned()),
            _ => {}
        }
    }
    q
}

async fn get_message(
    State(st): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    RawQuery(raw): RawQuery,
) -> Result<Json<Value>, GmailError> {
    let mut st = st.0.lock().unwrap();
    let email = authed_email(&st, &headers)?;
    if let Some(e) = check_fail(&mut st, "GET", &format!("/gmail/v1/users/me/messages/{id}")) {
        return Err(e);
    }
    let q = parse_get_query(raw.as_deref().unwrap_or(""));
    record(&mut st, "GET", "messages.get", q.query_pairs());
    let mb = st
        .mailboxes
        .get(&email)
        .ok_or_else(|| GmailError::new(404, "notFound"))?;
    let m = mb
        .messages
        .get(&id)
        .ok_or_else(|| GmailError::new(404, "notFound"))?;

    let format = q.format.as_deref().unwrap_or("full");
    let mut body = json!({
        "id": m.id,
        "threadId": m.id,
        "labelIds": stable_labels(&m.labels),
        "snippet": snippet(m),
        "internalDate": m.internal_date.unix_timestamp_nanos().div_euclid(1_000_000).to_string(),
        "sizeEstimate": m.raw.len(),
    });
    match format {
        "minimal" => {}
        "metadata" => {
            let payload = mime::payload(&m.raw);
            let headers = payload["headers"].as_array().cloned().unwrap_or_default();
            let wanted: Vec<String> = q
                .metadata_headers
                .iter()
                .map(|s| s.to_lowercase())
                .collect();
            let filtered: Vec<Value> = headers
                .into_iter()
                .filter(|h| {
                    wanted.is_empty()
                        || h["name"]
                            .as_str()
                            .is_some_and(|n| wanted.contains(&n.to_lowercase()))
                })
                .collect();
            body["payload"] = json!({ "headers": filtered });
        }
        "full" => {
            body["payload"] = mime::payload(&m.raw);
        }
        _ => return Err(GmailError::new(400, "invalidArgument")),
    }
    Ok(Json(body))
}

fn stable_labels(labels: &BTreeSet<String>) -> Vec<String> {
    let mut system: Vec<String> = labels
        .iter()
        .filter(|l| !l.starts_with("Label_"))
        .cloned()
        .collect();
    system.sort();
    let mut user: Vec<String> = labels
        .iter()
        .filter(|l| l.starts_with("Label_"))
        .cloned()
        .collect();
    user.sort();
    system.extend(user);
    system
}

fn snippet(m: &StoredMessage) -> String {
    let text = String::from_utf8_lossy(&m.raw);
    // Crude: take the body after the blank line, strip tags, first 100 chars.
    let body = text.split("\r\n\r\n").nth(1).unwrap_or("");
    let stripped = body.replace("<p>", " ").replace("</p>", " ");
    let cleaned: String = stripped.chars().filter(|c| !c.is_control()).collect();
    cleaned.chars().take(100).collect()
}

#[derive(Deserialize)]
#[allow(non_snake_case)]
struct ModifyBody {
    #[serde(default)]
    addLabelIds: Vec<String>,
    #[serde(default)]
    removeLabelIds: Vec<String>,
}

async fn modify_message(
    State(st): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<ModifyBody>,
) -> Result<Json<Value>, GmailError> {
    let mut st = st.0.lock().unwrap();
    let email = require_scope(&st, &headers, GMAIL_MODIFY)?;
    record(&mut st, "POST", "messages.modify", vec![]);
    let mb = st
        .mailboxes
        .get_mut(&email)
        .ok_or_else(|| GmailError::new(404, "notFound"))?;
    let m = mb
        .messages
        .get_mut(&id)
        .ok_or_else(|| GmailError::new(404, "notFound"))?;
    for l in &body.addLabelIds {
        if !mb.labels.contains_key(l) {
            return Err(GmailError::new(400, "invalidArgument"));
        }
        m.labels.insert(l.clone());
    }
    for l in &body.removeLabelIds {
        if !mb.labels.contains_key(l) {
            return Err(GmailError::new(400, "invalidArgument"));
        }
        m.labels.remove(l);
    }
    Ok(Json(json!({
        "id": m.id,
        "threadId": m.id,
        "labelIds": stable_labels(&m.labels),
    })))
}

async fn trash_message(
    State(st): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<Value>, GmailError> {
    let mut st = st.0.lock().unwrap();
    let email = require_scope(&st, &headers, GMAIL_MODIFY)?;
    record(&mut st, "POST", "messages.trash", vec![]);
    let mb = st
        .mailboxes
        .get_mut(&email)
        .ok_or_else(|| GmailError::new(404, "notFound"))?;
    let m = mb
        .messages
        .get_mut(&id)
        .ok_or_else(|| GmailError::new(404, "notFound"))?;
    m.labels.insert("TRASH".to_owned());
    m.labels.remove("INBOX");
    Ok(Json(json!({
        "id": m.id,
        "threadId": m.id,
        "labelIds": stable_labels(&m.labels),
    })))
}

async fn untrash_message(
    State(st): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<Value>, GmailError> {
    let mut st = st.0.lock().unwrap();
    let email = require_scope(&st, &headers, GMAIL_MODIFY)?;
    record(&mut st, "POST", "messages.untrash", vec![]);
    let mb = st
        .mailboxes
        .get_mut(&email)
        .ok_or_else(|| GmailError::new(404, "notFound"))?;
    let m = mb
        .messages
        .get_mut(&id)
        .ok_or_else(|| GmailError::new(404, "notFound"))?;
    m.labels.remove("TRASH");
    Ok(Json(json!({
        "id": m.id,
        "threadId": m.id,
        "labelIds": stable_labels(&m.labels),
    })))
}

#[derive(Deserialize)]
struct SendBody {
    raw: String,
}

async fn send_message(
    State(st): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<SendBody>,
) -> Result<Json<Value>, GmailError> {
    let mut st = st.0.lock().unwrap();
    let email = require_scope(&st, &headers, GMAIL_MODIFY)?;
    record(&mut st, "POST", "messages.send", vec![]);
    let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(body.raw.as_bytes())
        .map_err(|_| GmailError::new(400, "invalidArgument"))?;
    let id = st.next_message_id(&email);
    let mb = st
        .mailboxes
        .get_mut(&email)
        .ok_or_else(|| GmailError::new(404, "notFound"))?;
    mb.messages.insert(
        id.clone(),
        StoredMessage {
            id: id.clone(),
            raw,
            labels: BTreeSet::from(["SENT".to_owned()]),
            internal_date: OffsetDateTime::now_utc(),
        },
    );
    mb.sent.push(mb.messages[&id].raw.clone());
    Ok(Json(json!({
        "id": id,
        "threadId": id,
        "labelIds": ["SENT"],
    })))
}

async fn list_labels(
    State(st): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, GmailError> {
    let mut st = st.0.lock().unwrap();
    let email = authed_email(&st, &headers)?;
    record(&mut st, "GET", "labels.list", vec![]);
    let mb = st
        .mailboxes
        .get(&email)
        .ok_or_else(|| GmailError::new(404, "notFound"))?;
    let labels: Vec<Value> = mb
        .labels
        .values()
        .map(|l| json!({ "id": l.id, "name": l.name, "type": l.kind }))
        .collect();
    Ok(Json(json!({ "labels": labels })))
}

async fn get_label(
    State(st): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<Value>, GmailError> {
    let st = st.0.lock().unwrap();
    let email = authed_email(&st, &headers)?;
    let mb = st
        .mailboxes
        .get(&email)
        .ok_or_else(|| GmailError::new(404, "notFound"))?;
    let label = mb
        .labels
        .get(&id)
        .ok_or_else(|| GmailError::new(404, "notFound"))?;
    let total = mb
        .messages
        .values()
        .filter(|m| m.labels.contains(&id))
        .count();
    let unread = mb
        .messages
        .values()
        .filter(|m| m.labels.contains(&id) && m.labels.contains("UNREAD"))
        .count();
    Ok(Json(json!({
        "id": label.id,
        "name": label.name,
        "type": label.kind,
        "messagesTotal": total,
        "messagesUnread": unread,
    })))
}

#[derive(Deserialize)]
struct CreateLabelBody {
    name: String,
}

async fn create_label(
    State(st): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<CreateLabelBody>,
) -> Result<Json<Value>, GmailError> {
    let mut st = st.0.lock().unwrap();
    let email = require_scope(&st, &headers, GMAIL_MODIFY)?;
    record(&mut st, "POST", "labels.create", vec![]);
    // The race switch fires once, simulating another client creating the same
    // label between this client's list and its create: the label is made, the
    // response is 409.
    let race = st.label_create_race.remove(&email);
    let clash = st
        .mailboxes
        .get(&email)
        .ok_or_else(|| GmailError::new(404, "notFound"))?
        .labels
        .values()
        .any(|l| l.name.eq_ignore_ascii_case(&body.name));
    if race || clash {
        // The race: create the label anyway, then answer 409.
        if race {
            let id = st.next_user_label_id(&email);
            let mb = st
                .mailboxes
                .get_mut(&email)
                .ok_or_else(|| GmailError::new(404, "notFound"))?;
            mb.labels.insert(
                id.clone(),
                super::state::Label {
                    id,
                    name: body.name.clone(),
                    kind: "user".to_owned(),
                },
            );
        }
        return Err(GmailError::new(409, "alreadyExists"));
    }
    let id = st.next_user_label_id(&email);
    let mb = st
        .mailboxes
        .get_mut(&email)
        .ok_or_else(|| GmailError::new(404, "notFound"))?;
    mb.labels.insert(
        id.clone(),
        super::state::Label {
            id: id.clone(),
            name: body.name.clone(),
            kind: "user".to_owned(),
        },
    );
    Ok(Json(json!({ "id": id, "name": body.name, "type": "user" })))
}

async fn get_profile(
    State(st): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, GmailError> {
    let st = st.0.lock().unwrap();
    let email = authed_email(&st, &headers)?;
    let mb = st
        .mailboxes
        .get(&email)
        .ok_or_else(|| GmailError::new(404, "notFound"))?;
    let total = mb.messages.len();
    Ok(Json(json!({
        "emailAddress": email,
        "messagesTotal": total,
        "threadsTotal": total,
        "historyId": "1",
    })))
}

async fn delete_message(
    State(st): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<Value>, GmailError> {
    let mut st = st.0.lock().unwrap();
    let _ = authed_email(&st, &headers)?;
    record_delete(
        &mut st,
        "DELETE",
        &format!("/gmail/v1/users/me/messages/{id}"),
    );
    Err(GmailError::new(500, "backendError"))
}

async fn batch_delete(
    State(st): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, GmailError> {
    let mut st = st.0.lock().unwrap();
    let _ = authed_email(&st, &headers)?;
    record_delete(&mut st, "POST", "/gmail/v1/users/me/messages/batchDelete");
    Err(GmailError::new(500, "backendError"))
}

async fn delete_thread(
    State(st): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<Value>, GmailError> {
    let mut st = st.0.lock().unwrap();
    let _ = authed_email(&st, &headers)?;
    record_delete(
        &mut st,
        "DELETE",
        &format!("/gmail/v1/users/me/threads/{id}"),
    );
    Err(GmailError::new(500, "backendError"))
}
