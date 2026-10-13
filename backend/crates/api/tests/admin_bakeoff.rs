//! T-908b bake-off endpoint service integration tests: API-ADM-10 to API-ADM-14
//! (S7 5.13; S2 CL-04 AC6 to AC8; S10 9.4 STAT-7; S5 EXP-3; ASVS V1.2.1, V3.2.1,
//! V5.4.1, V8.2.1).
//!
//! Everything runs through the real router, the real handlers and the in-memory
//! fakes with the virtual clock, so the report is built from real
//! `classifier_eval` records, the CSV is the real attachment and a snapshot is a
//! real document that can be read back after the records are gone.
//!
//! The fixed data set is 120 records: 12 participants, one day, both models
//! answering, half the swipes junk and half wanted. Its small cells are
//! suppressed, and `stat_7_csv_matches_json` checks the CSV against the JSON
//! cell by cell, missing cells included.

#![allow(
    clippy::too_many_lines,
    clippy::items_after_statements,
    clippy::similar_names,
    clippy::needless_pass_by_value,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::missing_panics_doc
)]

use std::sync::Arc;

use api::bakeoff::csv::{CSV_HEADER, REPORT_FILENAME, SNAPSHOT_FILENAME};
use api::classify::eval::EVAL_TTL_DAYS;
use api::routes::admin_bakeoff::SNAPSHOT_LIMIT;
use api::session::store::SessionService;
use api::state::AppState;
use api::{app_state, build_router, config::ApiConfig};
use axum::body::Body;
use axum::http::{header, HeaderMap, Method, Request, StatusCode};
use axum::response::Response;
use domain::bakeoff::MIN_CELL_SIZE;
use domain::Provider;
use domain::{MessageClass, UserId};
use obs::{Pseudonymiser, Sensitive};
use ports::store::{BakeoffSnapshotRecord, EvalId, SnapshotId, UserPseudoId};
use ports::{
    AgeBucket, ClassifierEvalRecord, Clock, EvalHeaderFacts, EvalOutcome, HeaderRulesResult,
    KeyService, ModelPrediction, Precondition, Rng, ServerStore, SwipeDirection, TextTokensBucket,
    UserRecord,
};
use serde_json::Value;
use time::{Date, Duration, Month, OffsetDateTime, Time};
use tower::ServiceExt;
use uuid::Uuid;

type TestResult = Result<(), Box<dyn std::error::Error>>;

const ORIGIN: &str = "https://mailtinder.test";
const LOG_KEY: &[u8] = b"fake-log-key";
const EMAIL_KEY: &[u8] = b"fake-email-key";
/// The fixed data set's day.
const DAY: (i32, Month, u8) = (2026, Month::October, 5);
/// Distinct participants in the fixed data set.
const PARTICIPANTS: u32 = 12;
/// Records per participant, so every published cell has a denominator.
const PER_PARTICIPANT: u32 = 10;

/// The scoped log capture is per-thread; serialise the tests that assert on a
/// captured security event so a neighbour cannot blank it out.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn config() -> Result<ApiConfig, Box<dyn std::error::Error>> {
    Ok(ApiConfig::new(
        ORIGIN.to_owned(),
        "fake-client".into(),
        Sensitive::new(LOG_KEY.to_vec()),
        Sensitive::new(EMAIL_KEY.to_vec()),
    )?)
}

/// Noon UTC on the fixed data set's day, plus `offset_s`.
fn at(offset_s: i64) -> OffsetDateTime {
    let day = Date::from_calendar_date(DAY.0, DAY.1, DAY.2).unwrap_or(Date::MIN);
    let noon = OffsetDateTime::new_utc(day, Time::from_hms(12, 0, 0).unwrap_or(Time::MIDNIGHT));
    noon + Duration::seconds(offset_s)
}

/// A day in October 2026 as a query date.
fn date(number: u8) -> String {
    format!("2026-10-{number:02}")
}

/// The whole fixed range.
fn whole_day() -> String {
    format!("from={}&to={}", date(5), date(5))
}

/// One signed-in session's cookie and CSRF token.
struct Authed {
    cookie: String,
    csrf: String,
}

struct World {
    app: AppState,
    fakes: testkit::Fakes,
    admin: UserId,
    victim: UserId,
    doomed: UserId,
    stranger: UserId,
}

impl World {
    /// The fixed world: an admin, three ordinary users and the fixed report data.
    async fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let (ports, fakes) = testkit::fake_ports();
        let app = app_state(Arc::new(ports), Arc::new(config()?));
        let admin = seed_user(&fakes, true).await?;
        let victim = seed_user(&fakes, false).await?;
        let doomed = seed_user(&fakes, false).await?;
        let stranger = seed_user(&fakes, false).await?;
        let world = Self {
            app,
            fakes,
            admin,
            victim,
            doomed,
            stranger,
        };
        world.seed_fixed_data_set().await?;
        Ok(world)
    }

    fn router(&self) -> axum::Router {
        build_router(self.app.clone())
    }

    /// A signed-in session; `recent_auth_at` is the step-up state.
    async fn authed(
        &self,
        user: UserId,
        recent_auth_at: Option<OffsetDateTime>,
    ) -> Result<Authed, Box<dyn std::error::Error>> {
        let (cookie, record) = SessionService::new(&self.app)
            .establish(None, &user, recent_auth_at)
            .await?;
        Ok(Authed {
            cookie: cookie
                .0
                .to_str()?
                .split(';')
                .next()
                .unwrap_or_default()
                .to_owned(),
            csrf: record.record.csrf_token.clone(),
        })
    }

    async fn admin_session(&self) -> Result<Authed, Box<dyn std::error::Error>> {
        self.authed(self.admin, Some(self.fakes.clock.now())).await
    }

    /// `GET /api/v1/admin/bakeoff` (API-ADM-10).
    async fn get_report(
        &self,
        who: &Authed,
        query: &str,
        accept: Option<&str>,
    ) -> Result<Response, Box<dyn std::error::Error>> {
        let mut req = Request::builder()
            .method(Method::GET)
            .uri(format!("/api/v1/admin/bakeoff?{query}"))
            .header(header::COOKIE, who.cookie.as_str());
        if let Some(accept) = accept {
            req = req.header(header::ACCEPT, accept);
        }
        Ok(self.router().oneshot(req.body(Body::empty())?).await?)
    }

    /// `POST /api/v1/admin/bakeoff/snapshots` (API-ADM-11).
    async fn save_snapshot(
        &self,
        who: &Authed,
        body: &str,
    ) -> Result<Response, Box<dyn std::error::Error>> {
        let req = Request::builder()
            .method(Method::POST)
            .uri("/api/v1/admin/bakeoff/snapshots")
            .header(header::ORIGIN, ORIGIN)
            .header(header::COOKIE, who.cookie.as_str())
            .header("x-csrf-token", who.csrf.as_str())
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_owned()))?;
        Ok(self.router().oneshot(req).await?)
    }

    /// `GET /api/v1/admin/bakeoff/snapshots` (API-ADM-13).
    async fn list_snapshots(
        &self,
        who: &Authed,
        query: &str,
    ) -> Result<Response, Box<dyn std::error::Error>> {
        let uri = if query.is_empty() {
            "/api/v1/admin/bakeoff/snapshots".to_owned()
        } else {
            format!("/api/v1/admin/bakeoff/snapshots?{query}")
        };
        let req = Request::builder()
            .method(Method::GET)
            .uri(uri)
            .header(header::COOKIE, who.cookie.as_str())
            .body(Body::empty())?;
        Ok(self.router().oneshot(req).await?)
    }

    /// `GET /api/v1/admin/bakeoff/snapshots/{id}` (API-ADM-12).
    async fn get_snapshot(
        &self,
        who: &Authed,
        id: &str,
        accept: Option<&str>,
    ) -> Result<Response, Box<dyn std::error::Error>> {
        let mut req = Request::builder()
            .method(Method::GET)
            .uri(format!("/api/v1/admin/bakeoff/snapshots/{id}"))
            .header(header::COOKIE, who.cookie.as_str());
        if let Some(accept) = accept {
            req = req.header(header::ACCEPT, accept);
        }
        Ok(self.router().oneshot(req.body(Body::empty())?).await?)
    }

    /// `DELETE /api/v1/admin/bakeoff/snapshots/{id}` (API-ADM-14).
    async fn delete_snapshot(
        &self,
        who: &Authed,
        id: &str,
    ) -> Result<Response, Box<dyn std::error::Error>> {
        let req = Request::builder()
            .method(Method::DELETE)
            .uri(format!("/api/v1/admin/bakeoff/snapshots/{id}"))
            .header(header::ORIGIN, ORIGIN)
            .header(header::COOKIE, who.cookie.as_str())
            .header("x-csrf-token", who.csrf.as_str())
            .body(Body::empty())?;
        Ok(self.router().oneshot(req).await?)
    }

    /// Save the fixed report with `who` and return the response body. The
    /// session is the caller's: a second sign-in for one user ends the first
    /// (S7 3.2), so a helper that signs in again would strand the caller's
    /// cookie.
    async fn saved(&self, who: &Authed, name: &str) -> Result<Value, Box<dyn std::error::Error>> {
        let resp = self.save_snapshot(who, &save_body(name)).await?;
        assert_eq!(resp.status(), StatusCode::CREATED, "the save succeeds");
        json_of(resp).await
    }

    /// Every stored eval record count.
    async fn snapshot_count(&self) -> Result<u64, Box<dyn std::error::Error>> {
        Ok(self.app.ports.store.bakeoff_snapshots().count().await?)
    }

    /// Every stored `classifier_eval` record, paged: the store caps one page at
    /// its own maximum.
    async fn eval_records(&self) -> Result<Vec<ClassifierEvalRecord>, Box<dyn std::error::Error>> {
        let from = OffsetDateTime::UNIX_EPOCH;
        let to = from + Duration::days(365_000);
        let mut records = Vec::new();
        let mut after = None;
        loop {
            let page = self
                .fakes
                .store
                .classifier_eval()
                .range(
                    from,
                    to,
                    ports::PageRequest {
                        limit: ports::store::MAX_PAGE,
                        after: after.clone(),
                    },
                )
                .await?;
            records.extend(page.items.iter().cloned());
            match page.next {
                Some(next) => after = Some(next),
                None => return Ok(records),
            }
        }
    }

    /// The stored snapshot document.
    async fn stored(
        &self,
        id: Uuid,
    ) -> Result<Option<BakeoffSnapshotRecord>, Box<dyn std::error::Error>> {
        Ok(self
            .fakes
            .store
            .bakeoff_snapshots()
            .get(&SnapshotId(id))
            .await?
            .map(|v| v.record))
    }

    /// The fixed 120-record data set: 12 participants, both models answering,
    /// every version `1`, every record expiring after `EVAL_TTL_DAYS`.
    async fn seed_fixed_data_set(&self) -> TestResult {
        let total = u128::from(PARTICIPANTS) * u128::from(PER_PARTICIPANT);
        for i in 0..total {
            let participant = u32::try_from(i % u128::from(PARTICIPANTS))?;
            self.put_eval_row(
                EvalId(Uuid::from_u128(1_000 + i)),
                participant,
                MessageClass::List,
                "1",
                i,
            )
            .await?;
        }
        Ok(())
    }

    /// A second batch under another input version, for the `409` case.
    async fn seed_other_version(&self) -> TestResult {
        for i in 0..10_u128 {
            let participant = u32::try_from(i % u128::from(PARTICIPANTS))?;
            self.put_eval_row(
                EvalId(Uuid::from_u128(9_000 + i)),
                participant,
                MessageClass::List,
                "2",
                i,
            )
            .await?;
        }
        Ok(())
    }

    /// One eval record: the header rules call every class `list` for even rows
    /// and `personal` for odd ones, Gemini answers `list` and Jev `personal`, so
    /// the confusion table, the pairs and the trend all have real counts. Every
    /// third swipe is a junk label, the rest wanted.
    async fn put_eval_row(
        &self,
        eval_id: EvalId,
        participant: u32,
        class: MessageClass,
        version: &str,
        i: u128,
    ) -> TestResult {
        let class = if i % 2 == 0 {
            class
        } else {
            MessageClass::Personal
        };
        let direction = if i % 3 == 0 {
            SwipeDirection::Left
        } else {
            SwipeDirection::Right
        };
        let created_at = at(i64::try_from(i % 600).unwrap_or_default());
        let record = ClassifierEvalRecord {
            eval_id,
            user_pseudo_id: UserPseudoId(format!("pseudo-p{participant}")),
            created_at,
            expires_at: created_at + Duration::days(EVAL_TTL_DAYS),
            header_rules: HeaderRulesResult { class, score: 90 },
            gemini: Some(prediction(
                "gemini@flash-lite",
                MessageClass::List,
                120,
                150,
            )),
            jev: Some(prediction("jev@1.13.0", MessageClass::Personal, 400, 200)),
            outcome: EvalOutcome {
                direction,
                undone: false,
                time_to_swipe_ms: 1_200,
            },
            header_facts: EvalHeaderFacts {
                has_list_unsubscribe: i % 2 == 0,
                dkim_covers_list_unsubscribe: i % 2 == 0,
                has_list_unsubscribe_post: false,
                has_list_id: i % 2 == 0,
                has_feedback_id: false,
                precedence_bulk: i % 2 == 0,
                auto_submitted: false,
                from_authenticated: true,
            },
            provider: Provider::Gmail,
            age_bucket: AgeBucket::Under7d,
            text_tokens_bucket: TextTokensBucket::Under100,
            lang_is_english: true,
            input_version: version.to_owned(),
            question_version: "1".to_owned(),
            price_version: "1".to_owned(),
        };
        self.fakes
            .store
            .classifier_eval()
            .put(&record, Precondition::MustNotExist)
            .await?;
        Ok(())
    }

    /// Delete every eval record for a pseudonymous ID (the opt-out path), and
    /// report the user's own record (the account-deletion path).
    async fn remove_participant(&self, pseudo: &str) -> Result<u64, Box<dyn std::error::Error>> {
        Ok(self
            .app
            .ports
            .store
            .classifier_eval()
            .delete_for_users(&[UserPseudoId(pseudo.to_owned())])
            .await?)
    }

    /// Delete the user document, as account deletion does (S5).
    async fn delete_user(&self, user: UserId) -> TestResult {
        self.fakes
            .store
            .users()
            .delete(&user, Precondition::None)
            .await?;
        Ok(())
    }
}

/// A valid empty report, for the snapshots a test seeds directly into the
/// store (the read routes parse `report_json`).
fn empty_report_json() -> Result<String, Box<dyn std::error::Error>> {
    let query = domain::bakeoff::report::ReportQuery {
        from: Date::from_calendar_date(DAY.0, DAY.1, DAY.2)?,
        to: Date::from_calendar_date(DAY.0, DAY.1, DAY.2)?,
        input_version: None,
        question_version: None,
        price_version: None,
        pool_versions: false,
    };
    let report = domain::bakeoff::report::build_report(
        &[],
        &query,
        &domain::bakeoff::prices::default_prices(),
    )?;
    Ok(serde_json::to_string(&report)?)
}

/// The API-ADM-11 body for the fixed range.
fn save_body(name: &str) -> String {
    format!(
        r#"{{"name":"{name}","query":{{"from":"{}","to":"{}"}}}}"#,
        date(5),
        date(5)
    )
}

async fn seed_user(
    fakes: &testkit::Fakes,
    is_admin: bool,
) -> Result<UserId, Box<dyn std::error::Error>> {
    let user = UserId::new(fakes.rng.uuid_v4());
    let wrapped = fakes.keys.new_user_key(&user).await?;
    fakes
        .store
        .users()
        .put(
            &UserRecord {
                user_id: user,
                created_at: fakes.clock.now(),
                is_admin,
                wrapped_data_key: wrapped,
                experiments_consent_version: None,
                experiments_opted_in_at: None,
            },
            Precondition::MustNotExist,
        )
        .await?;
    Ok(user)
}

fn prediction(version: &str, class: MessageClass, latency_ms: u32, tokens: u32) -> ModelPrediction {
    ModelPrediction {
        class: Some(class),
        score: Some(70),
        confidence: Some(0.9),
        probabilities: None,
        model_version: version.to_owned(),
        latency_ms,
        input_tokens: Some(tokens),
        error_code: None,
    }
}

async fn json_of(resp: Response) -> Result<Value, Box<dyn std::error::Error>> {
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await?;
    Ok(serde_json::from_slice(&bytes)?)
}

async fn text_of(resp: Response) -> Result<String, Box<dyn std::error::Error>> {
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await?;
    Ok(String::from_utf8(bytes.to_vec())?)
}

/// The `code` of a problem body.
fn code_of(body: &Value) -> &str {
    body.get("code").and_then(Value::as_str).unwrap_or_default()
}

/// The pseudonymous ID a log line carries for `user`.
fn pseudo_of(user: &UserId) -> String {
    Pseudonymiser::new(Sensitive::new(LOG_KEY.to_vec()))
        .pseudo_id(&user.0)
        .as_str()
        .to_owned()
}

/// The parsed `security` lines of a capture (S5 logs).
fn security_events(text: &str) -> Vec<Value> {
    let mut events = Vec::new();
    for line in text.lines() {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if value.get("event").and_then(Value::as_str) == Some("security") {
            events.push(value);
        }
    }
    events
}

fn field<'a>(value: &'a Value, key: &str) -> &'a str {
    value.get(key).and_then(Value::as_str).unwrap_or_default()
}

// ---------------------------------------------------------------------------
// STAT-7, CL-04 AC7: the CSV is the JSON, cell for cell
// ---------------------------------------------------------------------------

/// One CSV row, its four number cells parsed.
#[derive(Debug, PartialEq)]
struct CsvRow {
    section: String,
    model: String,
    segment: String,
    metric: String,
    cells: [Option<f64>; 4],
}

impl CsvRow {
    fn key(&self) -> String {
        format!(
            "{}/{}/{}/{}",
            self.section, self.model, self.segment, self.metric
        )
    }
}

/// A small RFC 4180 reader: enough for this file, which has no bare newlines
/// inside fields.
fn parse_csv(text: &str) -> Vec<CsvRow> {
    let mut fields: Vec<String> = Vec::new();
    let mut row: Vec<String> = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if quoted {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    field.push('"');
                    chars.next();
                } else {
                    quoted = false;
                }
            } else {
                field.push(c);
            }
            continue;
        }
        match c {
            '"' if field.is_empty() => quoted = true,
            ',' => row.push(std::mem::take(&mut field)),
            '\n' => {
                row.push(std::mem::take(&mut field));
                fields.push(std::mem::take(&mut row).join("\u{1}"));
            }
            '\r' => {}
            _ => field.push(c),
        }
    }
    if !field.is_empty() || !row.is_empty() {
        row.push(field);
        fields.push(row.join("\u{1}"));
    }
    fields
        .into_iter()
        .skip(1)
        .filter(|line| !line.is_empty())
        .map(|line| {
            let parts: Vec<String> = line.split('\u{1}').map(ToString::to_string).collect();
            let cell = |i: usize| -> Option<f64> {
                parts
                    .get(i)
                    .filter(|s| !s.is_empty())
                    .and_then(|s| s.parse::<f64>().ok())
            };
            CsvRow {
                section: parts.first().cloned().unwrap_or_default(),
                model: parts.get(1).cloned().unwrap_or_default(),
                segment: parts.get(2).cloned().unwrap_or_default(),
                metric: parts.get(3).cloned().unwrap_or_default(),
                cells: [cell(4), cell(5), cell(6), cell(7)],
            }
        })
        .collect()
}

/// A number as the tidy table writes it: `{}` of the `f64`, so `1.0` is `1`.
fn num(v: &Value) -> String {
    v.as_f64().map(|x| x.to_string()).unwrap_or_default()
}

/// One expected row: a value and, where the report publishes them, bounds and a
/// denominator. The cell count is the table's own (S7 5.13).
#[allow(clippy::too_many_arguments)]
fn push(
    rows: &mut Vec<CsvRow>,
    section: &str,
    model: &str,
    segment: &str,
    metric: &str,
    value: Option<&Value>,
    lower: Option<&Value>,
    upper: Option<&Value>,
    n: Option<&Value>,
) {
    let cell = |v: Option<&Value>| v.filter(|v| !v.is_null()).and_then(Value::as_f64);
    rows.push(CsvRow {
        section: section.to_owned(),
        model: model.to_owned(),
        segment: segment.to_owned(),
        metric: metric.to_owned(),
        cells: [cell(value), cell(lower), cell(upper), cell(n)],
    });
}

/// A row with a value and nothing else.
fn plain_row(
    rows: &mut Vec<CsvRow>,
    section: &str,
    model: &str,
    segment: &str,
    metric: &str,
    value: Option<&Value>,
) {
    push(
        rows, section, model, segment, metric, value, None, None, None,
    );
}

/// A rate row: value, Wilson bounds and denominator.
fn rate_row(
    rows: &mut Vec<CsvRow>,
    section: &str,
    model: &str,
    segment: &str,
    metric: &str,
    rate: Option<&Value>,
) {
    let rate = rate.filter(|v| !v.is_null());
    let at = |key: &str| rate.and_then(|r| r.get(key));
    push(
        rows,
        section,
        model,
        segment,
        metric,
        at("value"),
        at("lower"),
        at("upper"),
        at("n"),
    );
}

/// Every row the tidy table must hold, derived from the report JSON by the
/// document's own table (S7 5.13 `CSV`). Written out again here on purpose: the
/// test is only worth something if it does not share the writer's walk.
fn expected_rows(report: &Value) -> Vec<CsvRow> {
    let mut rows = Vec::new();
    let at = |key: &str| report.get(key);

    // summary
    plain_row(
        &mut rows,
        "summary",
        "",
        "",
        "labelled_swipes",
        at("labelled_swipes"),
    );
    let participants = at("participants").unwrap_or(&Value::Null);
    plain_row(
        &mut rows,
        "summary",
        "",
        "",
        "participants",
        participants.get("count"),
    );
    rate_row(
        &mut rows,
        "summary",
        "",
        "",
        "top_contributor_share",
        participants.get("top_contributor_share"),
    );

    let models = at("models")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for model in &models {
        let name = field(model, "model");
        let metric = |key: &str| model.get(key);
        plain_row(&mut rows, "models", name, "", "cards", metric("cards"));
        plain_row(
            &mut rows,
            "models",
            name,
            "",
            "valid_answers",
            metric("valid_answers"),
        );
        rate_row(
            &mut rows,
            "models",
            name,
            "",
            "accuracy",
            metric("accuracy"),
        );
        rate_row(
            &mut rows,
            "models",
            name,
            "",
            "junk_precision",
            metric("junk_precision"),
        );
        rate_row(
            &mut rows,
            "models",
            name,
            "",
            "junk_recall",
            metric("junk_recall"),
        );
        plain_row(&mut rows, "models", name, "", "junk_f1", metric("junk_f1"));
        rate_row(
            &mut rows,
            "models",
            name,
            "",
            "false_junk_rate",
            metric("false_junk_rate"),
        );
        let calibration = metric("calibration").unwrap_or(&Value::Null);
        plain_row(&mut rows, "models", name, "", "ece", calibration.get("ece"));
        let latency = metric("latency").unwrap_or(&Value::Null);
        plain_row(
            &mut rows,
            "models",
            name,
            "",
            "p50_ms",
            latency.get("p50_ms"),
        );
        plain_row(
            &mut rows,
            "models",
            name,
            "",
            "p95_ms",
            latency.get("p95_ms"),
        );
        rate_row(
            &mut rows,
            "models",
            name,
            "",
            "timeout_rate",
            metric("timeout_rate"),
        );
        rate_row(
            &mut rows,
            "models",
            name,
            "",
            "error_rate",
            metric("error_rate"),
        );
        plain_row(
            &mut rows,
            "models",
            name,
            "",
            "input_tokens",
            metric("input_tokens"),
        );
        plain_row(
            &mut rows,
            "models",
            name,
            "",
            "cost_per_1000_messages_usd",
            metric("cost_per_1000_messages_usd"),
        );
    }

    for model in &models {
        let name = field(model, "model");
        let bins = model
            .get("calibration")
            .and_then(|c| c.get("bins"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        for bin in &bins {
            let segment = format!(
                "bin={}-{}",
                num(bin.get("confidence_from").unwrap_or(&Value::Null)),
                num(bin.get("confidence_to").unwrap_or(&Value::Null))
            );
            plain_row(
                &mut rows,
                "calibration",
                name,
                &segment,
                "count",
                bin.get("count"),
            );
            plain_row(
                &mut rows,
                "calibration",
                name,
                &segment,
                "observed_accuracy",
                bin.get("observed_accuracy"),
            );
        }
    }
    for model in &models {
        let name = field(model, "model");
        let bins = model
            .get("latency")
            .and_then(|l| l.get("histogram"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        for bin in &bins {
            let upper = match bin.get("to_ms") {
                Some(Value::Number(n)) => n.to_string(),
                _ => "inf".to_owned(),
            };
            let segment = format!(
                "bin={}-{}",
                num(bin.get("from_ms").unwrap_or(&Value::Null)),
                upper
            );
            plain_row(
                &mut rows,
                "latency",
                name,
                &segment,
                "count",
                bin.get("count"),
            );
        }
    }

    let pairs = at("paired")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for pair in &pairs {
        let name = format!("{}|{}", field(pair, "model_a"), field(pair, "model_b"));
        let metric = |key: &str| pair.get(key);
        plain_row(
            &mut rows,
            "paired",
            &name,
            "",
            "n_paired",
            metric("n_paired"),
        );
        plain_row(
            &mut rows,
            "paired",
            &name,
            "",
            "only_a_correct",
            metric("only_a_correct"),
        );
        plain_row(
            &mut rows,
            "paired",
            &name,
            "",
            "only_b_correct",
            metric("only_b_correct"),
        );
        plain_row(
            &mut rows,
            "paired",
            &name,
            "",
            "both_correct",
            metric("both_correct"),
        );
        plain_row(
            &mut rows,
            "paired",
            &name,
            "",
            "neither_correct",
            metric("neither_correct"),
        );
        plain_row(
            &mut rows,
            "paired",
            &name,
            "",
            "mcnemar_exact_p",
            metric("mcnemar_exact_p"),
        );
        let difference = metric("accuracy_difference").unwrap_or(&Value::Null);
        push(
            &mut rows,
            "paired",
            &name,
            "",
            "accuracy_difference",
            difference.get("value"),
            difference.get("lower"),
            difference.get("upper"),
            None,
        );
    }
    let agreements = at("agreement")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for agreement in &agreements {
        let name = format!(
            "{}|{}",
            field(agreement, "model_a"),
            field(agreement, "model_b")
        );
        rate_row(
            &mut rows,
            "agreement",
            &name,
            "",
            "rate",
            agreement.get("rate"),
        );
    }

    let segments = at("by_segment")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for segment in &segments {
        let model = field(segment, "model");
        let cell = format!(
            "{}={}",
            field(segment, "dimension"),
            field(segment, "value")
        );
        rate_row(
            &mut rows,
            "by_segment",
            model,
            &cell,
            "accuracy",
            segment.get("accuracy"),
        );
        rate_row(
            &mut rows,
            "by_segment",
            model,
            &cell,
            "false_junk_rate",
            segment.get("false_junk_rate"),
        );
    }

    let confusion = at("confusion")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for cell in &confusion {
        let segment = format!(
            "class={},label={}",
            field(cell, "class"),
            field(cell, "label")
        );
        plain_row(
            &mut rows,
            "confusion",
            field(cell, "model"),
            &segment,
            "count",
            cell.get("count"),
        );
    }

    let trend = at("trend")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for point in &trend {
        let segment = format!("date={}", field(point, "date"));
        rate_row(
            &mut rows,
            "trend",
            field(point, "model"),
            &segment,
            "accuracy",
            point.get("accuracy"),
        );
    }

    rows
}

/// Every number in the CSV equals the JSON's, and the CSV holds exactly the rows
/// the table asks for, suppressed cells included (STAT-7, CL-04 AC7).
fn assert_csv_matches_json(route: &str, csv: &str, report: &Value) {
    assert!(
        csv.starts_with(CSV_HEADER),
        "the header line is the fixed one"
    );
    let actual = parse_csv(csv);
    let expected = expected_rows(report);
    let keys: Vec<String> = expected.iter().map(CsvRow::key).collect();
    let unique: std::collections::BTreeSet<&String> = keys.iter().collect();
    assert_eq!(
        unique.len(),
        keys.len(),
        "the table has exactly one row per number"
    );
    let mut seen: std::collections::BTreeMap<String, &CsvRow> = std::collections::BTreeMap::new();
    for row in &actual {
        assert!(
            seen.insert(row.key(), row).is_none(),
            "one row for {}",
            row.key()
        );
    }
    for row in &expected {
        let found = seen
            .remove(&row.key())
            .unwrap_or_else(|| panic!("the CSV is missing row {}", row.key()));
        assert_eq!(
            found.cells,
            row.cells,
            "{route}: the JSON's numbers for {}",
            row.key()
        );
    }
    assert!(
        seen.is_empty(),
        "{route}: the CSV has rows the report does not: {:?}",
        seen.keys().collect::<Vec<_>>()
    );
}

/// STAT-7: for one fixed data set, every CSV number equals the JSON's, on
/// API-ADM-10 and on API-ADM-12, suppressed rows included.
#[tokio::test]
async fn stat_7_csv_matches_json() -> TestResult {
    let w = World::new().await?;
    let admin = w.admin_session().await?;

    // API-ADM-10.
    let json = json_of(
        w.get_report(&admin, &whole_day(), Some("application/json"))
            .await?,
    )
    .await?;
    let csv = text_of(w.get_report(&admin, &whole_day(), Some("text/csv")).await?).await?;
    assert_csv_matches_json("ADM-10", &csv, &json);

    // API-ADM-12: the same numbers, straight out of the stored snapshot.
    let saved = w.saved(&admin, "Blog draft").await?;
    let id = field(&saved, "snapshot_id").to_owned();
    let snapshot_json = json_of(
        w.get_snapshot(&admin, &id, Some("application/json"))
            .await?,
    )
    .await?;
    let snapshot_csv = text_of(w.get_snapshot(&admin, &id, Some("text/csv")).await?).await?;
    assert_csv_matches_json("ADM-12", &snapshot_csv, &snapshot_json["report"]);
    Ok(())
}

/// STAT-7: nothing a request carried reaches a header, and the file name is the
/// server's own constant.
#[tokio::test]
async fn stat_7_csv_filename_fixed() -> TestResult {
    let w = World::new().await?;
    let admin = w.admin_session().await?;
    let hostile = [
        "%22quote%22",
        "%0D%0A",
        "..%2F..%2Fetc",
        "%E2%82%AC",
        "a%20b",
    ];
    for value in hostile {
        let query = format!(
            "{}&input_version={value}&question_version={value}",
            whole_day()
        );
        let resp = w.get_report(&admin, &query, Some("text/csv")).await?;
        assert_headers_are_fixed(resp.headers(), value, REPORT_FILENAME);
    }
    Ok(())
}

/// A response never carries a request value in a header, and its attachment
/// name is exactly the constant.
fn assert_headers_are_fixed(headers: &HeaderMap, hostile: &str, filename: &str) {
    let decoded = url::form_urlencoded::parse(hostile.as_bytes())
        .map(|(_, v)| v.into_owned())
        .collect::<String>();
    let expected = format!("attachment; filename=\"{filename}\"");
    for (name, value) in headers {
        let Ok(text) = value.to_str() else {
            panic!("a header value is not ASCII: {name}");
        };
        if name.as_str() == "content-disposition" {
            assert_eq!(text, expected, "the name is the server's own constant");
            continue;
        }
        for needle in [hostile, decoded.as_str(), "\r", "\n", "€"] {
            assert!(
                needle.is_empty() || !text.contains(needle),
                "header {name} carries {needle:?}: {text:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// EXP-3: the snapshot holds aggregates only, and never moves
// ---------------------------------------------------------------------------

/// EXP-3 (S5): a stored snapshot holds no pseudonymous ID, no eval ID, no
/// account ID and no cell below the minimum size.
#[tokio::test]
async fn exp_3_snapshot_holds_no_ids() -> TestResult {
    let w = World::new().await?;
    let admin = w.admin_session().await?;
    let saved = w.saved(&admin, "First month").await?;
    let id = Uuid::parse_str(field(&saved, "snapshot_id"))?;
    let record = w.stored(id).await?.ok_or("the stored snapshot")?;

    let stored = format!("{}{}", record.report_json, record.query_json);
    // The pseudonymous IDs the records carried.
    for participant in 0..PARTICIPANTS {
        assert!(
            !stored.contains(&format!("pseudo-p{participant}")),
            "no pseudonymous ID in the snapshot"
        );
    }
    // The eval IDs and the users' own IDs.
    for i in 0..PARTICIPANTS * PER_PARTICIPANT {
        let eval_id = Uuid::from_u128(1_000 + u128::from(i)).to_string();
        assert!(!stored.contains(&eval_id), "no eval ID in the snapshot");
    }
    for user in [w.admin, w.victim, w.doomed, w.stranger] {
        assert!(
            !stored.contains(&user.0.to_string()),
            "no account ID in the snapshot"
        );
    }

    // No count below `min_cell_size` is published (S7 5.13 suppression).
    let report: Value = serde_json::from_str(&record.report_json)?;
    assert_eq!(report["min_cell_size"], Value::from(MIN_CELL_SIZE));
    for (key, value) in every_number(&report) {
        if is_count(&key) {
            let n = value.as_u64().unwrap_or_default();
            assert!(
                n >= MIN_CELL_SIZE,
                "a published count is below the minimum: {key} = {n}"
            );
        }
    }
    Ok(())
}

/// Every `(key, number)` pair in a JSON document, the key shorthand for nested
/// numbers (the object's own key where there is one).
fn every_number(value: &Value) -> Vec<(String, Value)> {
    let mut out = Vec::new();
    walk_numbers(value, "", &mut out);
    out
}

fn walk_numbers(value: &Value, key: &str, out: &mut Vec<(String, Value)>) {
    match value {
        Value::Number(_) => out.push((key.to_owned(), value.clone())),
        Value::Array(items) => {
            for item in items {
                walk_numbers(item, key, out);
            }
        }
        Value::Object(map) => {
            for (name, item) in map {
                walk_numbers(item, name, out);
            }
        }
        _ => {}
    }
}

/// The keys S7 5.13 publishes counts under.
fn is_count(key: &str) -> bool {
    matches!(
        key,
        "n" | "count"
            | "cards"
            | "valid_answers"
            | "labelled_swipes"
            | "n_paired"
            | "only_a_correct"
            | "only_b_correct"
            | "both_correct"
            | "neither_correct"
            | "participants"
            | "resamples"
    )
}

/// EXP-3 (S5): saving, opting a user out and deleting another account leaves the
/// snapshot byte-identical, and it is never recomputed.
#[tokio::test]
async fn exp_3_snapshot_unchanged_after_opt_out_and_deletion() -> TestResult {
    let w = World::new().await?;
    let admin = w.admin_session().await?;
    let saved = w.saved(&admin, "Before the opt-out").await?;
    let id = Uuid::parse_str(field(&saved, "snapshot_id"))?;
    let before = w.stored(id).await?.ok_or("the stored snapshot")?;

    // One participant opts out: their raw records are deleted.
    let removed = w.remove_participant("pseudo-p3").await?;
    assert!(removed > 0, "the opt-out deletes raw records");
    // Another account is deleted: its records and its user document go.
    w.remove_participant("pseudo-p7").await?;
    w.delete_user(w.doomed).await?;

    let after = w.stored(id).await?.ok_or("the snapshot survives")?;
    assert_eq!(after.report_json, before.report_json, "never recomputed");
    assert_eq!(after.query_json, before.query_json);
    assert_eq!(after.labelled_swipes, before.labelled_swipes);

    let served = text_of(w.get_snapshot(&admin, &id.to_string(), None).await?).await?;
    assert!(
        served.contains(&after.report_json),
        "the served report is the stored bytes, never recomputed"
    );
    let read = serde_json::from_str::<Value>(&served)?;
    assert_eq!(
        read["report"]["labelled_swipes"], saved["report"]["labelled_swipes"],
        "the stored body is served unchanged"
    );
    Ok(())
}

/// CL-04 AC6: a snapshot survives the eval records' 180-day expiry.
#[tokio::test]
async fn cl_04_ac6_snapshot_survives_eval_expiry() -> TestResult {
    let w = World::new().await?;
    let admin = w.admin_session().await?;
    let saved = w.saved(&admin, "Published figures").await?;
    let id = Uuid::parse_str(field(&saved, "snapshot_id"))?;
    let before = w.stored(id).await?.ok_or("the stored snapshot")?;
    assert!(!w.eval_records().await?.is_empty(), "records to expire");

    // 181 days on: every record is past its TTL and the sweep takes them.
    w.fakes.clock.advance(Duration::days(181));
    let now = w.fakes.clock.now();
    loop {
        let due = w
            .app
            .ports
            .store
            .classifier_eval()
            .expires_by(now, ports::store::MAX_PAGE)
            .await?;
        if due.is_empty() {
            break;
        }
        for eval_id in due {
            w.app
                .ports
                .store
                .classifier_eval()
                .delete(&eval_id, Precondition::None)
                .await?;
        }
    }
    assert_eq!(w.eval_records().await?.len(), 0, "the raw records are gone");

    let after = w.stored(id).await?.ok_or("the snapshot is kept")?;
    assert_eq!(after.report_json, before.report_json);
    // The old session expired with the clock; a fresh one still lists it.
    let later = w.admin_session().await?;
    let list = json_of(w.list_snapshots(&later, "").await?).await?;
    assert_eq!(list["snapshots"].as_array().map(Vec::len), Some(1));
    Ok(())
}

/// CL-04 AC7: the snapshot's CSV carries the snapshot's own figures.
#[tokio::test]
async fn cl_04_ac7_snapshot_csv_matches_snapshot_json() -> TestResult {
    let w = World::new().await?;
    let admin = w.admin_session().await?;
    let live = json_of(w.get_report(&admin, &whole_day(), None).await?).await?;
    let saved = w.saved(&admin, "Export").await?;
    let id = field(&saved, "snapshot_id").to_owned();
    assert_eq!(
        saved["report"], live,
        "the snapshot is the live report, suppressed"
    );
    let csv = text_of(w.get_snapshot(&admin, &id, Some("text/csv")).await?).await?;
    assert_csv_matches_json("ADM-12", &csv, &saved["report"]);
    Ok(())
}

/// CL-04 AC8 (S7 3.6): saving and deleting need a fresh Google sign-in; nothing
/// is written or removed without one.
#[tokio::test]
async fn cl_04_ac8_save_and_delete_need_step_up() -> TestResult {
    let w = World::new().await?;
    // The snapshot to delete is seeded directly: the test needs no sign-in to
    // get it there, and a second sign-in for the admin would end the first
    // (S7 3.2).
    let id = Uuid::from_u128(40_001);
    let record = BakeoffSnapshotRecord {
        snapshot_id: SnapshotId(id),
        name: "Step-up target".to_owned(),
        created_at: at(0),
        labelled_swipes: 120,
        query_json: "{}".to_owned(),
        report_json: empty_report_json()?,
    };
    w.fakes
        .store
        .bakeoff_snapshots()
        .put(&record, Precondition::MustNotExist)
        .await?;

    // No step-up: the session is an admin's, but its sign-in is stale.
    let stale = w.authed(w.admin, None).await?;
    let save = w
        .save_snapshot(&stale, &save_body("Without step-up"))
        .await?;
    assert_eq!(save.status(), StatusCode::FORBIDDEN);
    assert_eq!(code_of(&json_of(save).await?), "step_up_required");
    assert_eq!(w.snapshot_count().await?, 1, "nothing is written");

    let delete = w.delete_snapshot(&stale, &id.to_string()).await?;
    assert_eq!(delete.status(), StatusCode::FORBIDDEN);
    assert_eq!(code_of(&json_of(delete).await?), "step_up_required");
    assert!(
        w.stored(id).await?.is_some(),
        "nothing is removed without step-up"
    );

    // A fresh sign-in (the only kind that leaves the session stepped up) can
    // delete it; its log line is checked in `api_adm_14_delete_is_logged`.
    let admin = w.admin_session().await?;
    assert_eq!(
        w.delete_snapshot(&admin, &id.to_string()).await?.status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        w.snapshot_count().await?,
        0,
        "the stepped-up delete removes exactly the one snapshot"
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// S7 4 rows: versions_mixed, not_acceptable, snapshot_limit
// ---------------------------------------------------------------------------

/// `409 versions_mixed` with the versions present in the body (S7 5.13).
#[tokio::test]
async fn api_adm_10_versions_mixed_409() -> TestResult {
    let w = World::new().await?;
    w.seed_other_version().await?;
    let admin = w.admin_session().await?;
    let resp = w.get_report(&admin, &whole_day(), None).await?;
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    let body = json_of(resp).await?;
    assert_eq!(code_of(&body), "versions_mixed");
    assert_eq!(
        body["versions_present"]["input"],
        serde_json::json!(["1", "2"]),
        "both input versions are listed"
    );
    assert_eq!(
        body["versions_present"]["question"],
        serde_json::json!(["1"])
    );
    assert_eq!(body["versions_present"]["price"], serde_json::json!(["1"]));

    // Pooling the versions is allowed and says so in the query echo.
    let pooled = w
        .get_report(&admin, &format!("{}&pool_versions=true", whole_day()), None)
        .await?;
    assert_eq!(pooled.status(), StatusCode::OK);
    let report = json_of(pooled).await?;
    assert_eq!(report["query"]["pool_versions"], Value::Bool(true));
    Ok(())
}

/// `406 not_acceptable` when `Accept` offers neither JSON nor `text/csv`.
#[tokio::test]
async fn api_adm_10_not_acceptable_406() -> TestResult {
    let w = World::new().await?;
    let admin = w.admin_session().await?;
    let resp = w
        .get_report(&admin, &whole_day(), Some("application/xml"))
        .await?;
    assert_eq!(resp.status(), StatusCode::NOT_ACCEPTABLE);
    assert_eq!(code_of(&json_of(resp).await?), "not_acceptable");

    // A missing and a wildcard `Accept` both give the report.
    for accept in [None, Some("*/*")] {
        assert_eq!(
            w.get_report(&admin, &whole_day(), accept).await?.status(),
            StatusCode::OK
        );
    }
    Ok(())
}

/// `409 snapshot_limit` at 50 saved snapshots (S7 5.13 `[TUNABLE]`).
#[tokio::test]
async fn api_adm_11_snapshot_limit_409() -> TestResult {
    let w = World::new().await?;
    for i in 0..SNAPSHOT_LIMIT {
        let record = BakeoffSnapshotRecord {
            snapshot_id: SnapshotId(Uuid::from_u128(20_000 + u128::from(i))),
            name: format!("saved {i}"),
            created_at: at(i64::try_from(i).unwrap_or_default()),
            labelled_swipes: 120,
            query_json: "{}".to_owned(),
            report_json: "{}".to_owned(),
        };
        w.fakes
            .store
            .bakeoff_snapshots()
            .put(&record, Precondition::MustNotExist)
            .await?;
    }
    let admin = w.admin_session().await?;
    let body = format!(
        r#"{{"name":"one too many","query":{{"from":"{}","to":"{}"}}}}"#,
        date(5),
        date(5)
    );
    let resp = w.save_snapshot(&admin, &body).await?;
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    assert_eq!(code_of(&json_of(resp).await?), "snapshot_limit");
    assert_eq!(w.snapshot_count().await?, SNAPSHOT_LIMIT);
    Ok(())
}

/// API-ADM-11's name rule: 1 to 100 characters after plain-text cleaning.
#[tokio::test]
async fn api_adm_11_name_cleaned_or_refused() -> TestResult {
    let w = World::new().await?;
    let admin = w.admin_session().await?;
    // Whitespace and control characters only: empty after cleaning.
    let body = format!(
        r#"{{"name":"  \u200b\t\n  ","query":{{"from":"{}","to":"{}"}}}}"#,
        date(5),
        date(5)
    );
    let resp = w.save_snapshot(&admin, &body).await?;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    assert_eq!(code_of(&json_of(resp).await?), "invalid_request");
    assert_eq!(w.snapshot_count().await?, 0, "nothing is written");

    // Unknown fields are refused outright (S7 4).
    let body = format!(
        r#"{{"name":"ok","query":{{"from":"{}","to":"{}"}},"extra":1}}"#,
        date(5),
        date(5)
    );
    assert_eq!(
        w.save_snapshot(&admin, &body).await?.status(),
        StatusCode::BAD_REQUEST
    );
    Ok(())
}

/// API-ADM-13: newest first, summaries only, and a sealed cursor pages it.
#[tokio::test]
async fn api_adm_13_lists_newest_first() -> TestResult {
    let w = World::new().await?;
    for i in 0..3_u128 {
        let record = BakeoffSnapshotRecord {
            snapshot_id: SnapshotId(Uuid::from_u128(30_000 + i)),
            name: format!("snapshot {i}"),
            created_at: at(i64::try_from(i).unwrap_or_default() * 60),
            labelled_swipes: 120,
            query_json: "{}".to_owned(),
            report_json: "{}".to_owned(),
        };
        w.fakes
            .store
            .bakeoff_snapshots()
            .put(&record, Precondition::MustNotExist)
            .await?;
    }
    let admin = w.admin_session().await?;
    let page = json_of(w.list_snapshots(&admin, "").await?).await?;
    let names: Vec<String> = page["snapshots"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|s| field(s, "name").to_owned())
        .collect();
    assert_eq!(names, vec!["snapshot 2", "snapshot 1", "snapshot 0"]);
    let first = &page["snapshots"][0];
    assert!(
        first.get("report").is_none(),
        "summaries carry no report body"
    );
    assert_eq!(first["labelled_swipes"], Value::from(120));
    assert!(page["next_cursor"].is_null(), "one page holds them all");

    // A limit of one gives a cursor the next page accepts.
    let first_page = json_of(w.list_snapshots(&admin, "limit=1").await?).await?;
    assert_eq!(first_page["snapshots"].as_array().map(Vec::len), Some(1));
    let cursor = field(&first_page, "next_cursor").to_owned();
    assert!(!cursor.is_empty(), "more snapshots follow");
    let second_page = json_of(
        w.list_snapshots(&admin, &format!("limit=1&cursor={cursor}"))
            .await?,
    )
    .await?;
    assert_eq!(field(&second_page["snapshots"][0], "name"), "snapshot 1");

    // The cursor is bound to the admin who was given it.
    let stranger = w.authed(w.stranger, None).await?;
    assert_eq!(
        w.list_snapshots(&stranger, &format!("limit=1&cursor={cursor}"))
            .await?
            .status(),
        StatusCode::FORBIDDEN
    );
    Ok(())
}

/// API-ADM-14: deleting is `204`, a missing snapshot is `404`, and the delete is
/// logged.
#[tokio::test]
async fn api_adm_14_delete_is_logged() -> TestResult {
    let _serial = SERIAL.lock().await;
    let w = World::new().await?;
    let admin = w.admin_session().await?;
    let saved = w.saved(&admin, "Throwaway").await?;
    let id = field(&saved, "snapshot_id").to_owned();

    let clock = obs::arc(obs::FixedClock(w.fakes.clock.now()));
    let (capture, _guard) = obs::capture("api", clock);
    let resp = w.delete_snapshot(&admin, &id).await?;
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    assert_eq!(w.snapshot_count().await?, 0, "the snapshot is gone");
    let events = security_events(&capture.text());
    let event = events
        .iter()
        .find(|e| field(e, "action") == "snapshot_delete")
        .ok_or("a snapshot_delete security event")?;
    assert_eq!(field(event, "user_pseudo"), pseudo_of(&w.admin));
    assert_ne!(field(event, "request_id"), "");

    // Deleting it twice is a `404`, not a second success.
    let again = w.delete_snapshot(&admin, &id).await?;
    assert_eq!(again.status(), StatusCode::NOT_FOUND);
    assert_eq!(code_of(&json_of(again).await?), "not_found");
    Ok(())
}

/// Saving is logged once, pseudonymously (S6 7), and the read routes are not.
#[tokio::test]
async fn api_adm_11_save_is_logged() -> TestResult {
    let _serial = SERIAL.lock().await;
    let w = World::new().await?;
    let admin = w.admin_session().await?;
    let clock = obs::arc(obs::FixedClock(w.fakes.clock.now()));
    let (capture, _guard) = obs::capture("api", clock);
    let saved = w.saved(&admin, "Logged").await?;
    let id = field(&saved, "snapshot_id").to_owned();
    let _ = text_of(w.get_snapshot(&admin, &id, None).await?).await?;
    let _ = json_of(w.list_snapshots(&admin, "").await?).await?;

    let events = security_events(&capture.text());
    let event = events
        .iter()
        .find(|e| field(e, "action") == "snapshot_save")
        .ok_or("a snapshot_save security event")?;
    assert_eq!(field(event, "user_pseudo"), pseudo_of(&w.admin));
    assert_eq!(field(event, "outcome"), "success");
    assert!(
        !capture.text().contains("Logged"),
        "the name never reaches a log line"
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// ASVS
// ---------------------------------------------------------------------------

/// ASVS V3.2.1 (S7 5.13): the report and a snapshot are served as an attachment
/// with the CSV media type.
#[tokio::test]
async fn asvs_v3_2_1_csv_served_as_attachment() -> TestResult {
    let w = World::new().await?;
    let admin = w.admin_session().await?;
    let saved = w.saved(&admin, "Attachment").await?;
    let id = field(&saved, "snapshot_id").to_owned();
    for (resp, filename) in [
        (
            w.get_report(&admin, &whole_day(), Some("text/csv")).await?,
            REPORT_FILENAME,
        ),
        (
            w.get_snapshot(&admin, &id, Some("text/csv")).await?,
            SNAPSHOT_FILENAME,
        ),
    ] {
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers()[header::CONTENT_TYPE],
            "text/csv; charset=utf-8"
        );
        assert_eq!(
            resp.headers()[header::CONTENT_DISPOSITION],
            format!("attachment; filename=\"{filename}\"")
        );
        let body = text_of(resp).await?;
        assert!(body.starts_with(CSV_HEADER));
    }
    Ok(())
}

/// ASVS V5.4.1 (S7 5.13): the file name is the server's, never a query
/// parameter's and never a snapshot's name.
#[tokio::test]
async fn asvs_v5_4_1_csv_filename_not_from_input() -> TestResult {
    let w = World::new().await?;
    let admin = w.admin_session().await?;
    for hostile in ["..%2Fetc%2Fpasswd", "%22quoted%22", "%0D%0ASet-Cookie%3Ax"] {
        let query = format!("{}&price_version={hostile}", whole_day());
        let resp = w.get_report(&admin, &query, Some("text/csv")).await?;
        assert_headers_are_fixed(resp.headers(), hostile, REPORT_FILENAME);
    }

    // A snapshot whose name is hostile still downloads under the fixed name.
    let saved = w.saved(&admin, "../../etc/passwd").await?;
    let id = field(&saved, "snapshot_id").to_owned();
    let stored = w.stored(Uuid::parse_str(&id)?).await?.ok_or("stored")?;
    assert!(
        w.app
            .ports
            .store
            .bakeoff_snapshots()
            .get(&SnapshotId(Uuid::parse_str(&id)?))
            .await?
            .is_some(),
        "the snapshot exists"
    );
    assert!(!stored.name.is_empty(), "the name was kept as text");
    let resp = w.get_snapshot(&admin, &id, Some("text/csv")).await?;
    assert_headers_are_fixed(resp.headers(), "..%2F..%2Fetc", SNAPSHOT_FILENAME);
    assert_eq!(
        resp.headers()[header::CONTENT_DISPOSITION],
        format!("attachment; filename=\"{SNAPSHOT_FILENAME}\"")
    );
    Ok(())
}

/// ASVS V8.2.1 (S7 table): every bake-off route refuses a non-admin, and the
/// refusal is logged.
#[tokio::test]
async fn asvs_v8_2_1_bakeoff_routes_refused_for_non_admin() -> TestResult {
    let _serial = SERIAL.lock().await;
    let w = World::new().await?;
    let admin = w.admin_session().await?;
    let saved = w.saved(&admin, "Admin only").await?;
    let id = field(&saved, "snapshot_id").to_owned();
    // The stranger is a different user, so their sign-in cannot end the
    // admin's session (S7 3.2).
    let non_admin = w.authed(w.stranger, Some(w.fakes.clock.now())).await?;

    let clock = obs::arc(obs::FixedClock(w.fakes.clock.now()));
    let (capture, _guard) = obs::capture("api", clock);
    let responses = vec![
        (
            "GET /admin/bakeoff",
            w.get_report(&non_admin, &whole_day(), None).await?,
        ),
        (
            "POST /admin/bakeoff/snapshots",
            w.save_snapshot(
                &non_admin,
                &format!(
                    r#"{{"name":"nope","query":{{"from":"{}","to":"{}"}}}}"#,
                    date(5),
                    date(5)
                ),
            )
            .await?,
        ),
        (
            "GET /admin/bakeoff/snapshots",
            w.list_snapshots(&non_admin, "").await?,
        ),
        (
            "GET /admin/bakeoff/snapshots/{id}",
            w.get_snapshot(&non_admin, &id, None).await?,
        ),
        (
            "DELETE /admin/bakeoff/snapshots/{id}",
            w.delete_snapshot(&non_admin, &id).await?,
        ),
    ];
    for (route, resp) in responses {
        assert_eq!(resp.status(), StatusCode::FORBIDDEN, "{route}");
        assert_eq!(code_of(&json_of(resp).await?), "forbidden", "{route}");
    }
    assert_eq!(
        w.snapshot_count().await?,
        1,
        "nothing was written or removed"
    );
    let events = security_events(&capture.text());
    assert_eq!(
        events
            .iter()
            .filter(|e| field(e, "action") == "authz_failure")
            .count(),
        5,
        "one authz_failure per refused route"
    );
    for event in &events {
        assert_eq!(field(event, "outcome"), "refused");
        assert_eq!(field(event, "user_pseudo"), pseudo_of(&w.stranger));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Aggregates only
// ---------------------------------------------------------------------------

/// S10 9.4 (S7 5.13): the response is aggregates only; no array in it can be one
/// entry per record.
#[tokio::test]
async fn bakeoff_report_aggregates_only() -> TestResult {
    let w = World::new().await?;
    let records = w.eval_records().await?.len();
    assert_eq!(
        records,
        usize::try_from(PARTICIPANTS * PER_PARTICIPANT)?,
        "the fixed data set"
    );
    let admin = w.admin_session().await?;
    let report = json_of(w.get_report(&admin, &whole_day(), None).await?).await?;
    for (path, length) in array_lengths(&report) {
        assert_ne!(
            length, records,
            "an array of one entry per record at {path}"
        );
        assert!(
            length < records,
            "no array is as long as the data set at {path}"
        );
    }
    Ok(())
}

/// Every array in a document, as `(path, length)`.
fn array_lengths(value: &Value) -> Vec<(String, usize)> {
    let mut out = Vec::new();
    walk_arrays(value, "", &mut out);
    out
}

fn walk_arrays(value: &Value, path: &str, out: &mut Vec<(String, usize)>) {
    match value {
        Value::Array(items) => {
            out.push((path.to_owned(), items.len()));
            for (i, item) in items.iter().enumerate() {
                walk_arrays(item, &format!("{path}[{i}]"), out);
            }
        }
        Value::Object(map) => {
            for (name, item) in map {
                walk_arrays(item, &format!("{path}.{name}"), out);
            }
        }
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// The read routes need no step-up, the writes do
// ---------------------------------------------------------------------------

/// The two writes are refused without step-up at the extractor, before any
/// budget or body is read (S7 3.6); the three reads are not stepped up.
#[tokio::test]
async fn api_adm_routes_read_only_need_admin() -> TestResult {
    let w = World::new().await?;
    let stale_admin = w.authed(w.admin, None).await?;
    // One snapshot, seeded directly: the reads below need no stepped-up session.
    let id = Uuid::from_u128(40_002);
    let record = BakeoffSnapshotRecord {
        snapshot_id: SnapshotId(id),
        name: "Reads are open".to_owned(),
        created_at: at(0),
        labelled_swipes: 120,
        query_json: "{}".to_owned(),
        report_json: empty_report_json()?,
    };
    w.fakes
        .store
        .bakeoff_snapshots()
        .put(&record, Precondition::MustNotExist)
        .await?;
    assert_eq!(
        w.get_report(&stale_admin, &whole_day(), None)
            .await?
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        w.list_snapshots(&stale_admin, "").await?.status(),
        StatusCode::OK
    );
    assert_eq!(
        w.get_snapshot(&stale_admin, &id.to_string(), None)
            .await?
            .status(),
        StatusCode::OK
    );
    Ok(())
}
