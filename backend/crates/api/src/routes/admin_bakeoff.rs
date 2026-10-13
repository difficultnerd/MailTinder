//! T-908b: API-ADM-10 to API-ADM-14, the bake-off report endpoints
//! (S7 5.13, S7 3.6, S7 4 rows `versions_mixed`, `snapshot_limit`,
//! `not_acceptable`, S7 6 rows "Bake-off report", "Bake-off snapshots saved",
//! "Bake-off snapshots deleted"; S2 CL-04 AC6 to AC8, STAT-7; S5
//! `bakeoff_snapshots/{id}` and EXP-3; ASVS V1.2.1, V3.2.1, V5.4.1, V8.2.1).
//!
//! An admin reads the live report as JSON or as the tidy CSV (API-ADM-10), saves
//! it as a snapshot (API-ADM-11), lists snapshots (API-ADM-13), opens one
//! (API-ADM-12) and deletes one (API-ADM-14). Saving and deleting need a fresh
//! Google sign-in ([`SteppedUpAdmin`], CL-04 AC8) and are logged; reads need an
//! admin.
//!
//! A snapshot is the already suppressed report as JSON, plus its query and the
//! count it published: aggregates only, no per-user value, so it survives the
//! raw records' expiry, an opt-out and an account deletion, and it is never
//! recomputed (CL-04 AC6, S5 EXP-3). The CSV's file name is a server constant and
//! never anything a request carried (ASVS V5.4.1); its cells are formula-escaped
//! (ASVS V1.2.1) and its numbers are the JSON's (STAT-7).
//!
//! The two security events reuse the registered S5 actions `snapshot_save` and
//! `snapshot_delete`: S5's log fields have no slot for a snapshot ID, so the line
//! carries the pseudonymous admin ID, the action and the request ID, never a
//! snapshot ID or a name.

use std::collections::BTreeSet;

use axum::body::Body;
use axum::extract::{Path, RawQuery, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::Response;
use domain::bakeoff::prices::default_prices;
use domain::bakeoff::report::{build_report, BakeoffReport, EvalRow, ReportError, ReportQuery};
use domain::bakeoff::MIN_CELL_SIZE;
use domain::UserId;
use obs::{Pseudonymiser, SecurityEvent};
use ports::store::{BakeoffSnapshotRecord, SnapshotId};
use ports::{PageRequest, Precondition, StoreCursor, Versioned, WrappedKey};
use serde::{Deserialize, Serialize};
use time::{Date, Duration, OffsetDateTime};
use uuid::Uuid;

use crate::bakeoff::csv::{report_to_csv, REPORT_FILENAME, SNAPSHOT_FILENAME};
use crate::bakeoff::load::load_rows;
use crate::error::ApiError;
use crate::http::json::{json_ok, ApiJson};
use crate::http::request_id::RequestId;
use crate::limits::{policies, LimitSubject};
use crate::sealed::{SealedTokens, TokenType};
use crate::session::extract::{AdminSession, AuthedSession, SteppedUpAdmin};
use crate::state::AppState;
use crate::text::plain_text;

/// At most this many snapshots may exist (S7 5.13 `[TUNABLE]`).
pub const SNAPSHOT_LIMIT: u64 = 50;
/// The longest snapshot name, after plain-text cleaning (S7 5.13).
pub const NAME_MAX_CHARS: usize = 100;
/// The most a version filter may hold (S7 5.13, the `OpenAPI`'s `maxLength`).
pub const VERSION_MAX_CHARS: usize = 16;
/// The widest report range (S7 5.13 `[DEFAULT]`), in days.
pub const MAX_RANGE_DAYS: i64 = 366;
/// The default page size of the snapshot list (S7 2).
pub const LIST_LIMIT_DEFAULT: u32 = 20;
/// The largest page a client may ask for (S7 2).
pub const LIST_LIMIT_MAX: u32 = 50;
/// `[DEFAULT]` how long a snapshot page cursor lives (S7 2.1).
pub const CURSOR_TTL: Duration = Duration::hours(12);
/// The day format the report's `from` and `to` use (`format: date`).
const ISO_DATE: &[time::format_description::FormatItem<'_>] =
    time::macros::format_description!("[year]-[month]-[day]");

/// The API-ADM-11 request body.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SaveSnapshot {
    /// The name the admin will see, 1 to 100 characters of plain text.
    pub name: String,
    /// The report to save.
    pub query: ReportQuery,
}

/// The API-ADM-11 / API-ADM-12 body: the whole snapshot (S7 5.13 `Snapshot`).
#[derive(Serialize)]
pub struct SnapshotDto {
    /// The snapshot's ID.
    pub snapshot_id: Uuid,
    /// The name the admin gave it.
    pub name: String,
    /// When it was saved.
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    /// The suppressed count the report published, `null` below `min_cell_size`.
    pub labelled_swipes: Option<u64>,
    /// The stored report, never recomputed.
    pub report: BakeoffReport,
}

/// One API-ADM-13 row (S7 5.13 `SnapshotSummary`): no report body.
#[derive(Serialize)]
pub struct SnapshotSummaryDto {
    /// The snapshot's ID.
    pub snapshot_id: Uuid,
    /// The name the admin gave it.
    pub name: String,
    /// When it was saved.
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    /// The suppressed count, `null` below `min_cell_size`.
    pub labelled_swipes: Option<u64>,
}

/// The API-ADM-13 body.
#[derive(Serialize)]
pub struct SnapshotListDto {
    /// Newest first.
    pub snapshots: Vec<SnapshotSummaryDto>,
    /// A sealed cursor for the next page, `None` at the end.
    pub next_cursor: Option<String>,
}

/// The sealed cursor payload; `after` is the store's own cursor.
#[derive(Serialize, Deserialize)]
pub struct SnapshotCursor {
    /// The store cursor to resume after.
    pub after: String,
}

/// The `cursor` and `limit` of API-ADM-13, and nothing else.
#[derive(Debug)]
pub struct ListParams {
    /// A sealed cursor from an earlier page, or `None` to start.
    pub cursor: Option<String>,
    /// Snapshots wanted, 1 to [`LIST_LIMIT_MAX`].
    pub limit: u32,
}

impl ListParams {
    /// Parse `cursor` and `limit` (1 to 50, default 20); an unknown parameter or
    /// a bad value is `400 invalid_request` (S7 4).
    ///
    /// # Errors
    ///
    /// `ApiError::InvalidRequest` for an unknown parameter or a `limit` outside
    /// 1 to 50.
    pub fn parse(query: Option<&str>) -> Result<Self, ApiError> {
        let mut out = Self {
            cursor: None,
            limit: LIST_LIMIT_DEFAULT,
        };
        for (key, value) in url::form_urlencoded::parse(query.unwrap_or_default().as_bytes()) {
            match key.as_ref() {
                "cursor" => out.cursor = Some(value.into_owned()),
                "limit" => {
                    out.limit = value
                        .parse::<u32>()
                        .ok()
                        .filter(|n| (1..=LIST_LIMIT_MAX).contains(n))
                        .ok_or_else(|| invalid("/limit"))?;
                }
                _ => return Err(invalid("")),
            }
        }
        Ok(out)
    }
}

/// What the caller asked for (S7 5.13 `CSV`, row `not_acceptable`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Wanted {
    /// The report, or the snapshot, as JSON.
    Json,
    /// The tidy table.
    Csv,
}

impl Wanted {
    /// Content negotiation: `text/csv` gives the CSV; a missing, `*/*` or
    /// `application/json` `Accept` gives JSON; anything else is `406`.
    ///
    /// # Errors
    ///
    /// `ApiError::NotAcceptable` when nothing in `Accept` is on offer.
    fn of(headers: &HeaderMap) -> Result<Self, ApiError> {
        let Some(value) = headers.get(header::ACCEPT) else {
            return Ok(Self::Json);
        };
        let text = value.to_str().map_err(|_| ApiError::NotAcceptable)?;
        if text.trim().is_empty() {
            return Ok(Self::Json);
        }
        let mut json = false;
        for offer in text.split(',') {
            let media = offer
                .split(';')
                .next()
                .unwrap_or_default()
                .trim()
                .to_ascii_lowercase();
            match media.as_str() {
                "text/csv" => return Ok(Self::Csv),
                "*/*" | "application/json" => json = true,
                _ => {}
            }
        }
        if json {
            Ok(Self::Json)
        } else {
            Err(ApiError::NotAcceptable)
        }
    }
}

/// `GET /api/v1/admin/bakeoff` (API-ADM-10).
///
/// # Errors
///
/// `ApiError::Forbidden` for a non-admin (the extractor logs and refuses);
/// `ApiError::InvalidRequest` for a missing, malformed or too wide range or an
/// unknown parameter; `ApiError::NotAcceptable` when `Accept` offers neither JSON
/// nor `text/csv`; `ApiError::RateLimited` past 30 requests an hour;
/// `ApiError::VersionsMixed` when the range mixes input or question versions;
/// `ApiError::Internal` on a store failure.
pub async fn get_report(
    State(app): State<AppState>,
    admin: AdminSession,
    request_id: RequestId,
    headers: HeaderMap,
    RawQuery(raw): RawQuery,
) -> Result<Response, ApiError> {
    let wanted = Wanted::of(&headers)?;
    let query = report_query(raw.as_deref())?;
    // 30 an hour per admin, instance memory (S7 6).
    app.limits
        .check(
            &policies::BAKEOFF_REPORT,
            LimitSubject::User(&admin.0.user),
            request_id,
        )
        .await?;
    let report = report_for(&app, &query).await?;
    match wanted {
        Wanted::Json => Ok(json_ok(StatusCode::OK, &report)),
        Wanted::Csv => csv_response(&report_to_csv(&report), REPORT_FILENAME),
    }
}

/// `POST /api/v1/admin/bakeoff/snapshots` (API-ADM-11).
///
/// # Errors
///
/// `ApiError::Forbidden` for a non-admin, `ApiError::StepUpRequired` without a
/// fresh Google sign-in (both from the extractors; the admin check runs first);
/// `ApiError::RateLimited` past 20 saves a day; `ApiError::InvalidRequest` for a
/// name that is empty after plain-text cleaning or a query the report cannot take;
/// `ApiError::SnapshotLimit` at [`SNAPSHOT_LIMIT`]; `ApiError::VersionsMixed` for
/// a mixed range; `ApiError::Internal` on a store failure.
pub async fn save_snapshot(
    State(app): State<AppState>,
    SteppedUpAdmin(admin): SteppedUpAdmin,
    request_id: RequestId,
    ApiJson(body): ApiJson<SaveSnapshot>,
) -> Result<Response, ApiError> {
    // 20 a day per admin (S7 6).
    app.limits
        .check(
            &policies::SNAPSHOT_SAVE,
            LimitSubject::User(&admin.user),
            request_id,
        )
        .await?;

    // 1 to 100 characters of plain text; empty after cleaning is a client error.
    let name = plain_text(&body.name, NAME_MAX_CHARS);
    if name.is_empty() {
        return Err(invalid("/name"));
    }
    range_allowed(&body.query)?;

    if app.ports.store.bakeoff_snapshots().count().await? >= SNAPSHOT_LIMIT {
        return Err(ApiError::SnapshotLimit);
    }

    // The report exactly as API-ADM-10 builds it: already suppressed, and the
    // same `409 versions_mixed` rule.
    let report = report_for(&app, &body.query).await?;
    let created_at = app.ports.clock.now();
    let snapshot_id = SnapshotId(app.ports.rng.uuid_v4());
    let record = BakeoffSnapshotRecord {
        snapshot_id,
        name: name.clone(),
        created_at,
        labelled_swipes: report.labelled_swipes.unwrap_or(0),
        query_json: json_string(&body.query)?,
        report_json: json_string(&report)?,
    };
    app.ports
        .store
        .bakeoff_snapshots()
        .put(&record, Precondition::MustNotExist)
        .await?;

    saved_event(&app, &admin.user, request_id);
    Ok(json_ok(
        StatusCode::CREATED,
        &SnapshotDto {
            snapshot_id: snapshot_id.0,
            name,
            created_at,
            labelled_swipes: report.labelled_swipes,
            report,
        },
    ))
}

/// `GET /api/v1/admin/bakeoff/snapshots` (API-ADM-13).
///
/// # Errors
///
/// `ApiError::Forbidden` for a non-admin; `ApiError::InvalidRequest` for a bad
/// `limit`, cursor or unknown parameter; `ApiError::Unauthenticated` when the
/// admin's own record is gone; `ApiError::Internal` on a store or key failure.
pub async fn list_snapshots(
    State(app): State<AppState>,
    admin: AdminSession,
    RawQuery(raw): RawQuery,
) -> Result<Response, ApiError> {
    let params = ListParams::parse(raw.as_deref())?;
    let session = &admin.0;
    let Some(record) = app.ports.store.users().get(&session.user).await? else {
        return Err(ApiError::Unauthenticated);
    };
    let wrapped = record.record.wrapped_data_key;
    let after = open_cursor(&app, session, &wrapped, params.cursor.as_deref()).await?;
    let page = app
        .ports
        .store
        .bakeoff_snapshots()
        .list(PageRequest {
            limit: params.limit,
            after,
        })
        .await?;
    let snapshots = page.items.iter().map(summary_of).collect();
    let next_cursor = seal_cursor(&app, session, &wrapped, page.next).await?;
    Ok(json_ok(
        StatusCode::OK,
        &SnapshotListDto {
            snapshots,
            next_cursor,
        },
    ))
}

/// `GET /api/v1/admin/bakeoff/snapshots/{snapshot_id}` (API-ADM-12).
///
/// # Errors
///
/// `ApiError::Forbidden` for a non-admin; `ApiError::NotFound` for a missing or
/// malformed ID; `ApiError::NotAcceptable` when `Accept` offers neither JSON nor
/// `text/csv`; `ApiError::RateLimited` past the CSV budget; `ApiError::Internal`
/// when a stored document is unreadable.
pub async fn get_snapshot(
    State(app): State<AppState>,
    admin: AdminSession,
    request_id: RequestId,
    headers: HeaderMap,
    Path(snapshot_id): Path<String>,
) -> Result<Response, ApiError> {
    let wanted = Wanted::of(&headers)?;
    if wanted == Wanted::Csv {
        // The CSV of a snapshot spends the same report budget as API-ADM-10's.
        app.limits
            .check(
                &policies::BAKEOFF_REPORT,
                LimitSubject::User(&admin.0.user),
                request_id,
            )
            .await?;
    }
    let record = load_snapshot(&app, &snapshot_id).await?;
    let report = stored_report(&record)?;
    match wanted {
        Wanted::Json => Ok(json_ok(
            StatusCode::OK,
            &SnapshotDto {
                snapshot_id: record.snapshot_id.0,
                name: record.name,
                created_at: record.created_at,
                labelled_swipes: report.labelled_swipes,
                report,
            },
        )),
        Wanted::Csv => csv_response(&report_to_csv(&report), SNAPSHOT_FILENAME),
    }
}

/// `DELETE /api/v1/admin/bakeoff/snapshots/{snapshot_id}` (API-ADM-14).
///
/// # Errors
///
/// `ApiError::Forbidden` for a non-admin, `ApiError::StepUpRequired` without a
/// fresh Google sign-in; `ApiError::RateLimited` past 50 deletes a day;
/// `ApiError::NotFound` for a missing or malformed ID; `ApiError::Internal` on a
/// store failure.
pub async fn delete_snapshot(
    State(app): State<AppState>,
    SteppedUpAdmin(admin): SteppedUpAdmin,
    request_id: RequestId,
    Path(snapshot_id): Path<String>,
) -> Result<StatusCode, ApiError> {
    app.limits
        .check(
            &policies::SNAPSHOT_DELETE,
            LimitSubject::User(&admin.user),
            request_id,
        )
        .await?;
    let record = load_snapshot(&app, &snapshot_id).await?;
    app.ports
        .store
        .bakeoff_snapshots()
        .delete(&record.snapshot_id, Precondition::None)
        .await?;
    deleted_event(&app, &admin.user, request_id);
    Ok(StatusCode::NO_CONTENT)
}

/// Load one snapshot, or `404` (S7 4): a malformed ID is a missing snapshot.
///
/// # Errors
///
/// `ApiError::NotFound` for a malformed or unknown ID; `ApiError::Internal` on a
/// store failure.
async fn load_snapshot(app: &AppState, raw: &str) -> Result<BakeoffSnapshotRecord, ApiError> {
    let Ok(id) = Uuid::parse_str(raw) else {
        return Err(ApiError::NotFound);
    };
    app.ports
        .store
        .bakeoff_snapshots()
        .get(&SnapshotId(id))
        .await?
        .map(|v| v.record)
        .ok_or(ApiError::NotFound)
}

/// The stored report, read back and never recomputed (CL-04 AC6, EXP-3).
///
/// # Errors
///
/// `ApiError::Internal` when the stored document is unreadable.
fn stored_report(record: &BakeoffSnapshotRecord) -> Result<BakeoffReport, ApiError> {
    serde_json::from_str(&record.report_json).map_err(|_| ApiError::Internal)
}

/// One API-ADM-13 summary. `labelled_swipes` is `null` below `min_cell_size`,
/// which is how the stored suppressed report reads too (S7 5.13).
fn summary_of(snapshot: &Versioned<BakeoffSnapshotRecord>) -> SnapshotSummaryDto {
    SnapshotSummaryDto {
        snapshot_id: snapshot.record.snapshot_id.0,
        name: snapshot.record.name.clone(),
        created_at: snapshot.record.created_at,
        labelled_swipes: (snapshot.record.labelled_swipes >= MIN_CELL_SIZE)
            .then_some(snapshot.record.labelled_swipes),
    }
}

/// The rows in scope and the finished, suppressed report (S7 5.13 API-ADM-10).
///
/// # Errors
///
/// `ApiError::VersionsMixed` when the range mixes input or question versions and
/// pooling was not asked for; `ApiError::Internal` on a store failure.
async fn report_for(app: &AppState, query: &ReportQuery) -> Result<BakeoffReport, ApiError> {
    let rows = load_rows(app, query).await?;
    build_report(&rows, query, &default_prices()).map_err(|e| versions_mixed(&rows, e))
}

/// The `409 versions_mixed` error, carrying the versions present (S7 5.13).
///
/// The report builder owns the input and question versions; the price versions
/// never block pooling, so they are collected here for the body's completeness.
fn versions_mixed(rows: &[EvalRow], error: ReportError) -> ApiError {
    match error {
        ReportError::VersionsMixed { input, question } => {
            let price = rows
                .iter()
                .map(|row| row.price_version.clone())
                .collect::<BTreeSet<String>>()
                .into_iter()
                .collect();
            ApiError::VersionsMixed {
                input,
                question,
                price,
            }
        }
    }
}

/// The API-ADM-10 query string: `from` and `to` required, the three version
/// filters and `pool_versions` optional, nothing else (S7 4).
///
/// # Errors
///
/// `ApiError::InvalidRequest` for a missing or malformed date, a version filter
/// that is empty or not a short ASCII token, a `pool_versions` that is not a
/// boolean, an unknown parameter, `from` after `to`, or a range over
/// [`MAX_RANGE_DAYS`] days.
pub fn report_query(raw: Option<&str>) -> Result<ReportQuery, ApiError> {
    let mut from: Option<Date> = None;
    let mut to: Option<Date> = None;
    let mut input_version: Option<String> = None;
    let mut question_version: Option<String> = None;
    let mut price_version: Option<String> = None;
    let mut pool_versions = false;
    for (key, value) in url::form_urlencoded::parse(raw.unwrap_or_default().as_bytes()) {
        match key.as_ref() {
            "from" => from = Some(date_of(&value).ok_or_else(|| invalid("/from"))?),
            "to" => to = Some(date_of(&value).ok_or_else(|| invalid("/to"))?),
            "input_version" => input_version = Some(version_of(&value, "/input_version")?),
            "question_version" => {
                question_version = Some(version_of(&value, "/question_version")?);
            }
            "price_version" => price_version = Some(version_of(&value, "/price_version")?),
            "pool_versions" => {
                pool_versions = match value.as_ref() {
                    "true" => true,
                    "false" => false,
                    _ => return Err(invalid("/pool_versions")),
                };
            }
            _ => return Err(invalid("")),
        }
    }
    let query = ReportQuery {
        from: from.ok_or_else(|| invalid("/from"))?,
        to: to.ok_or_else(|| invalid("/to"))?,
        input_version,
        question_version,
        price_version,
        pool_versions,
    };
    range_allowed(&query)?;
    Ok(query)
}

/// The range rule shared by API-ADM-10 and API-ADM-11: `from` not after `to`,
/// and no more than [`MAX_RANGE_DAYS`] days between them (S7 5.13 `[DEFAULT]`).
///
/// # Errors
///
/// `ApiError::InvalidRequest` naming `/from` or `/to`.
fn range_allowed(query: &ReportQuery) -> Result<(), ApiError> {
    if query.from > query.to {
        return Err(invalid("/from"));
    }
    if (query.to - query.from).whole_days() > MAX_RANGE_DAYS {
        return Err(invalid("/to"));
    }
    Ok(())
}

/// A `format: date` day.
fn date_of(raw: &str) -> Option<Date> {
    Date::parse(raw, ISO_DATE).ok()
}

/// A version filter: a short printable ASCII token, so no request value can
/// carry a delimiter, a control character or a long string into a log line or a
/// header (S7 5.13 bounds it to 16 characters; ASVS V5.4.1).
fn version_of(raw: &str, field: &str) -> Result<String, ApiError> {
    let ok = !raw.is_empty()
        && raw.chars().count() <= VERSION_MAX_CHARS
        && raw.chars().all(|c| c.is_ascii_graphic());
    if ok {
        Ok(raw.to_owned())
    } else {
        Err(invalid(field))
    }
}

/// The tidy table as an attachment with the server's fixed file name (S7 5.13;
/// ASVS V3.2.1, V5.4.1).
///
/// # Errors
///
/// `ApiError::Internal` when the fixed header value cannot be built (it is
/// ASCII, so this cannot happen for the two constants).
fn csv_response(body: &str, filename: &'static str) -> Result<Response, ApiError> {
    let disposition = format!("attachment; filename=\"{filename}\"");
    let value = HeaderValue::from_str(&disposition).map_err(|_| ApiError::Internal)?;
    let mut response = Response::new(Body::from(body.to_owned()));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/csv; charset=utf-8"),
    );
    response
        .headers_mut()
        .insert(header::CONTENT_DISPOSITION, value);
    Ok(response)
}

/// One snapshot document as JSON, for the record's two string columns.
///
/// # Errors
///
/// `ApiError::Internal` when the report cannot be serialised (it can).
fn json_string<T: Serialize>(value: &T) -> Result<String, ApiError> {
    serde_json::to_string(value).map_err(|_| ApiError::Internal)
}

/// Open a presented page cursor; any failure is `400 invalid_request`.
///
/// # Errors
///
/// `ApiError::InvalidRequest` for a malformed, foreign or expired cursor;
/// `ApiError::Internal` when the key service is down.
async fn open_cursor(
    app: &AppState,
    session: &AuthedSession,
    wrapped: &WrappedKey,
    cursor: Option<&str>,
) -> Result<Option<StoreCursor>, ApiError> {
    let Some(token) = cursor else {
        return Ok(None);
    };
    let payload: SnapshotCursor = sealed_tokens(app)
        .open(
            TokenType::Cursor,
            &session.user,
            wrapped,
            &session.session_record_id,
            token,
        )
        .await
        .map_err(|e| match e {
            crate::sealed::TokenError::Unavailable => ApiError::Internal,
            _ => invalid("/cursor"),
        })?;
    Ok(Some(StoreCursor(payload.after)))
}

/// Seal the store's next cursor for the wire, or `None` at the end.
///
/// # Errors
///
/// `ApiError::Internal` on a key failure.
async fn seal_cursor(
    app: &AppState,
    session: &AuthedSession,
    wrapped: &WrappedKey,
    next: Option<StoreCursor>,
) -> Result<Option<String>, ApiError> {
    let Some(next) = next else {
        return Ok(None);
    };
    let token = sealed_tokens(app)
        .seal(
            TokenType::Cursor,
            &session.user,
            wrapped,
            &session.session_record_id,
            app.ports.clock.now() + CURSOR_TTL,
            &SnapshotCursor { after: next.0 },
        )
        .await
        .map_err(|_| ApiError::Internal)?;
    Ok(Some(token))
}

/// The sealed-token service, over the app's key service and clock.
fn sealed_tokens(app: &AppState) -> SealedTokens {
    SealedTokens::new(
        std::sync::Arc::clone(&app.ports.keys),
        std::sync::Arc::clone(&app.ports.clock),
    )
}

/// A field-named `400 invalid_request` (S7 4).
fn invalid(field: &str) -> ApiError {
    ApiError::InvalidRequest {
        fields: vec![field.to_owned()],
    }
}

/// The `snapshot_save` security event (S6 7). S5's log fields hold no snapshot
/// ID and no name, so the line carries the pseudonymous admin, the action and
/// the request ID.
fn saved_event(app: &AppState, admin: &UserId, request_id: RequestId) {
    snapshot_event(app, admin, "snapshot_save", request_id);
}

/// The `snapshot_delete` security event (S6 7).
fn deleted_event(app: &AppState, admin: &UserId, request_id: RequestId) {
    snapshot_event(app, admin, "snapshot_delete", request_id);
}

/// Log one snapshot change, pseudonymously (S6 7).
fn snapshot_event(app: &AppState, admin: &UserId, action: &'static str, request_id: RequestId) {
    obs::security_event(&SecurityEvent {
        action,
        outcome: "success",
        user: Some(Pseudonymiser::new(app.config.rate_key.clone()).pseudo_id(&admin.0)),
        request_id: Some(request_id.0),
        amr: None,
        provider: None,
        method: None,
    });
}
