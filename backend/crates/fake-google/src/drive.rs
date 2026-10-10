//! Drive appDataFolder routes for `fake-google` (T-205b).
//!
//! Route table (every route needs a token with `drive.appdata`, else 403
//! `insufficientPermissions`; a token with only Gmail scopes is refused):
//!
//! | Method and path | Behaviour |
//! | --- | --- |
//! | `GET /drive/v3/files` | requires `spaces=appDataFolder` (else 403 `insufficientScopes`); supports `q` of the form `name = '<n>' and trashed = false` only (other `q` gives 400); `pageSize`; returns `{"files":[{"id","name","modifiedTime"}]}` |
//! | `GET /drive/v2/files/{id}` | `{"id","etag","title","modifiedDate"}`; 404 if absent |
//! | `GET /drive/v3/files/{id}?alt=media` | raw bytes, `Content-Type: application/octet-stream`; without `alt=media` returns v3 metadata JSON |
//! | `POST /upload/drive/v3/files?uploadType=multipart` | body `multipart/related` (boundary from `Content-Type`): first part JSON `{"name","parents":["appDataFolder"]}`, second part the bytes. `parents` must be exactly `["appDataFolder"]`, else 403. Returns `{"id"}` |
//! | `PUT /upload/drive/v2/files/{id}?uploadType=media` | header `If-Match` required; equal to current etag: replace bytes, bump version, return `{"id","etag"}`; different: 412 `conditionNotMet`; missing header: 428 `preconditionRequired`; file absent: 404 |
//! | `DELETE /drive/v3/files/{id}` | 204; 404 if absent |

use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post, put};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use time::OffsetDateTime;

use super::errors::GmailError;
use super::gmail::{check_fail, record, AppState};
use super::state::{DriveFile, FakeState};
use super::tokens::DRIVE_APPDATA;

/// The maximum accepted upload body (5 MiB).
const MAX_UPLOAD: usize = 5 * 1024 * 1024;

/// Build the Drive router (mounted at the root; paths are absolute).
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/drive/v3/files", get(list_files))
        .route("/drive/v3/files/{id}", get(get_file_v3).delete(delete_file))
        .route("/drive/v2/files/{id}", get(get_file_v2))
        .route("/upload/drive/v3/files", post(create_file))
        .route("/upload/drive/v2/files/{id}", put(update_file))
}

/// The authenticated mailbox email, requiring the `drive.appdata` scope.
fn require_drive_scope(state: &FakeState, headers: &HeaderMap) -> Result<String, GmailError> {
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
    if !rec.scopes.contains(DRIVE_APPDATA) {
        return Err(GmailError::new(403, "insufficientPermissions"));
    }
    Ok(rec.mailbox.0.clone())
}

/// The current etag for a file: `"v<version>"` (quotes are part of the value).
fn etag(version: u64) -> String {
    format!("\"v{version}\"")
}

fn modified_time(f: &DriveFile) -> String {
    f.modified_time
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}

#[derive(Deserialize, Serialize)]
#[allow(non_snake_case)]
struct ListQuery {
    #[serde(default)]
    spaces: Option<String>,
    #[serde(default)]
    q: Option<String>,
    #[serde(default)]
    pageSize: Option<u32>,
}

async fn list_files(
    State(st): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<ListQuery>,
) -> Result<Json<Value>, GmailError> {
    let mut st = st.0.lock().unwrap();
    let email = require_drive_scope(&st, &headers)?;
    if let Some(e) = check_fail(&mut st, "GET", "/drive/v3/files") {
        return Err(e);
    }
    let query = query_pairs(&q);
    record(&mut st, "GET", "drive.files.list", query);
    if q.spaces.as_deref() != Some("appDataFolder") {
        return Err(GmailError::new(403, "insufficientScopes"));
    }
    if let Some(qs) = &q.q {
        if !is_valid_q(qs) {
            return Err(GmailError::new(400, "invalidArgument"));
        }
    }
    let mb = st
        .mailboxes
        .get(&email)
        .ok_or_else(|| GmailError::new(404, "notFound"))?;
    let page_size = q.pageSize.unwrap_or(100).min(1000) as usize;
    let mut files: Vec<&DriveFile> = mb.drive.files.values().filter(|f| !f.trashed).collect();
    files.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.id.cmp(&b.id)));
    let items: Vec<Value> = files
        .iter()
        .take(page_size)
        .map(|f| {
            json!({
                "id": f.id,
                "name": f.name,
                "modifiedTime": modified_time(f),
            })
        })
        .collect();
    Ok(Json(json!({ "files": items })))
}

fn is_valid_q(q: &str) -> bool {
    // Only `name = '<n>' and trashed = false` is supported.
    let q = q.trim();
    let Some((name_part, trash_part)) = q.split_once(" and ") else {
        return false;
    };
    let name_part = name_part.trim();
    let trash_part = trash_part.trim();
    if trash_part != "trashed = false" {
        return false;
    }
    let Some(rest) = name_part.strip_prefix("name = ") else {
        return false;
    };
    rest.starts_with('\'') && rest.ends_with('\'') && rest.len() >= 2
}

async fn get_file_v2(
    State(st): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<Value>, GmailError> {
    let mut st = st.0.lock().unwrap();
    let email = require_drive_scope(&st, &headers)?;
    if let Some(e) = check_fail(&mut st, "GET", "/drive/v2/files") {
        return Err(e);
    }
    record(&mut st, "GET", "drive.files.get.v2", vec![]);
    let mb = st
        .mailboxes
        .get(&email)
        .ok_or_else(|| GmailError::new(404, "notFound"))?;
    let f = mb
        .drive
        .files
        .get(&id)
        .ok_or_else(|| GmailError::new(404, "notFound"))?;
    Ok(Json(json!({
        "id": f.id,
        "etag": etag(f.version),
        "title": f.name,
        "modifiedDate": modified_time(f),
    })))
}

async fn get_file_v3(
    State(st): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(q): Query<MediaQuery>,
) -> Result<axum::response::Response, GmailError> {
    let mut st = st.0.lock().unwrap();
    let email = require_drive_scope(&st, &headers)?;
    if let Some(e) = check_fail(&mut st, "GET", "/drive/v3/files") {
        return Err(e);
    }
    let is_media = q.alt.as_deref() == Some("media");
    if is_media {
        record(
            &mut st,
            "GET",
            "drive.files.get.media",
            vec![("alt".into(), "media".into())],
        );
    } else {
        record(&mut st, "GET", "drive.files.get.v3", vec![]);
    }
    let mb = st
        .mailboxes
        .get(&email)
        .ok_or_else(|| GmailError::new(404, "notFound"))?;
    let f = mb
        .drive
        .files
        .get(&id)
        .ok_or_else(|| GmailError::new(404, "notFound"))?;
    if is_media {
        let bytes = f.bytes.clone();
        return Ok((
            StatusCode::OK,
            [("Content-Type", "application/octet-stream")],
            bytes,
        )
            .into_response());
    }
    Ok(Json(json!({
        "id": f.id,
        "name": f.name,
        "mimeType": "application/octet-stream",
        "modifiedTime": modified_time(f),
        "size": f.bytes.len().to_string(),
    }))
    .into_response())
}

#[derive(Deserialize)]
struct MediaQuery {
    #[serde(default)]
    alt: Option<String>,
}

#[derive(Deserialize)]
#[allow(non_snake_case)]
struct UploadQuery {
    #[serde(default)]
    uploadType: Option<String>,
}

async fn create_file(
    State(st): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<UploadQuery>,
    body: Bytes,
) -> Result<Json<Value>, GmailError> {
    let now = st.now();
    let mut st = st.0.lock().unwrap();
    let email = require_drive_scope(&st, &headers)?;
    if let Some(e) = check_fail(&mut st, "POST", "/upload/drive/v3/files") {
        return Err(e);
    }
    record(
        &mut st,
        "POST",
        "drive.files.create",
        vec![("uploadType".into(), "multipart".into())],
    );
    if q.uploadType.as_deref() != Some("multipart") {
        return Err(GmailError::new(400, "invalidArgument"));
    }
    if body.len() > MAX_UPLOAD {
        return Err(GmailError::new(413, "requestTooLarge"));
    }
    let boundary = multipart_boundary(&headers)?;
    let (meta, data) = parse_multipart(&body, &boundary)?;
    let meta: Value =
        serde_json::from_slice(&meta).map_err(|_| GmailError::new(400, "invalidArgument"))?;
    let name = meta
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| GmailError::new(400, "invalidArgument"))?;
    let parents = meta
        .get("parents")
        .and_then(|v| v.as_array())
        .ok_or_else(|| GmailError::new(400, "invalidArgument"))?;
    if parents.len() != 1 || parents[0].as_str() != Some("appDataFolder") {
        return Err(GmailError::new(403, "insufficientScopes"));
    }
    let mb = st.mailboxes.entry(email).or_default();
    mb.drive.next_id += 1;
    let id = format!("appdata-{}", mb.drive.next_id);
    mb.drive.files.insert(
        id.clone(),
        DriveFile {
            id: id.clone(),
            name: name.to_owned(),
            parents: vec!["appDataFolder".to_owned()],
            bytes: data,
            version: 1,
            modified_time: now,
            trashed: false,
        },
    );
    Ok(Json(json!({ "id": id })))
}

async fn update_file(
    State(st): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(q): Query<UploadQuery>,
    body: Bytes,
) -> Result<Json<Value>, GmailError> {
    let now = st.now();
    let mut st = st.0.lock().unwrap();
    let email = require_drive_scope(&st, &headers)?;
    if let Some(e) = check_fail(&mut st, "PUT", "/upload/drive/v2/files") {
        return Err(e);
    }
    record(
        &mut st,
        "PUT",
        "drive.files.update.v2",
        vec![("uploadType".into(), "media".into())],
    );
    if q.uploadType.as_deref() != Some("media") {
        return Err(GmailError::new(400, "invalidArgument"));
    }
    if body.len() > MAX_UPLOAD {
        return Err(GmailError::new(413, "requestTooLarge"));
    }
    let if_match = headers
        .get("if-match")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| GmailError::new(428, "preconditionRequired"))?;
    let mb = st
        .mailboxes
        .get_mut(&email)
        .ok_or_else(|| GmailError::new(404, "notFound"))?;
    let f = mb
        .drive
        .files
        .get_mut(&id)
        .ok_or_else(|| GmailError::new(404, "notFound"))?;
    if if_match != etag(f.version) {
        return Err(GmailError::new(412, "conditionNotMet"));
    }
    f.bytes = body.to_vec();
    f.version += 1;
    f.modified_time = now;
    Ok(Json(json!({ "id": f.id, "etag": etag(f.version) })))
}

async fn delete_file(
    State(st): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<StatusCode, GmailError> {
    let mut st = st.0.lock().unwrap();
    let email = require_drive_scope(&st, &headers)?;
    if let Some(e) = check_fail(&mut st, "DELETE", "/drive/v3/files") {
        return Err(e);
    }
    record(&mut st, "DELETE", "drive.files.delete", vec![]);
    let mb = st
        .mailboxes
        .get_mut(&email)
        .ok_or_else(|| GmailError::new(404, "notFound"))?;
    if mb.drive.files.remove(&id).is_none() {
        return Err(GmailError::new(404, "notFound"));
    }
    Ok(StatusCode::NO_CONTENT)
}

/// Extract the multipart boundary from the `Content-Type` header.
fn multipart_boundary(headers: &HeaderMap) -> Result<String, GmailError> {
    let ct = headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| GmailError::new(400, "invalidArgument"))?;
    let boundary = ct
        .split(';')
        .map(str::trim)
        .find_map(|p| p.strip_prefix("boundary="))
        .ok_or_else(|| GmailError::new(400, "invalidArgument"))?;
    // Strip surrounding quotes if present.
    let boundary = boundary.trim_matches('"');
    if boundary.is_empty() {
        return Err(GmailError::new(400, "invalidArgument"));
    }
    Ok(boundary.to_owned())
}

/// Parse a `multipart/related` body into (first-part bytes, second-part bytes).
///
/// Byte-safe: the second part is the app-folder file's raw bytes (sealed
/// ciphertext, arbitrary binary), so the body must be split on byte sequences.
/// A `String::from_utf8_lossy` pass would replace every invalid sequence with
/// U+FFFD and corrupt the stored file, making every later read unreadable.
fn parse_multipart(body: &[u8], boundary: &str) -> Result<(Vec<u8>, Vec<u8>), GmailError> {
    let delim = format!("--{boundary}");
    // parts[0] is the preamble; parts[1..] are the parts (last is the closing --).
    let mut contents = Vec::new();
    for part in split_on(body, delim.as_bytes()).into_iter().skip(1) {
        let part = part.strip_prefix(b"--".as_slice()).unwrap_or(part);
        let part = trim_start_crlf(part);
        if part.is_empty() {
            continue;
        }
        // Split headers from content at the first blank line.
        let Some(index) = find_subslice(part, b"\r\n\r\n") else {
            continue;
        };
        let content = &part[index + 4..];
        let content = content.strip_suffix(b"\r\n".as_slice()).unwrap_or(content);
        contents.push(content.to_vec());
    }
    if contents.len() < 2 {
        return Err(GmailError::new(400, "invalidArgument"));
    }
    Ok((contents[0].clone(), contents[1].clone()))
}

/// Split `haystack` on every non-overlapping occurrence of `needle`.
fn split_on<'a>(haystack: &'a [u8], needle: &[u8]) -> Vec<&'a [u8]> {
    let mut parts = Vec::new();
    let mut start = 0;
    let mut i = 0;
    while i + needle.len() <= haystack.len() {
        if &haystack[i..i + needle.len()] == needle {
            parts.push(&haystack[start..i]);
            i += needle.len();
            start = i;
        } else {
            i += 1;
        }
    }
    parts.push(&haystack[start..]);
    parts
}

/// The first index of `needle` in `haystack`, if any.
fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    (0..=haystack.len() - needle.len()).find(|&i| &haystack[i..i + needle.len()] == needle)
}

/// Leading `\r` and `\n` bytes removed.
fn trim_start_crlf(mut bytes: &[u8]) -> &[u8] {
    while let Some((&first, rest)) = bytes.split_first() {
        if first == b'\r' || first == b'\n' {
            bytes = rest;
        } else {
            break;
        }
    }
    bytes
}

fn query_pairs<T: serde::Serialize>(q: &T) -> Vec<(String, String)> {
    serde_json::to_value(q)
        .ok()
        .and_then(|v| v.as_object().cloned())
        .map(|m| {
            m.into_iter()
                .filter_map(|(k, v)| match v {
                    Value::String(s) => Some((k, s)),
                    Value::Null => None,
                    _ => Some((k, v.to_string())),
                })
                .collect()
        })
        .unwrap_or_default()
}
