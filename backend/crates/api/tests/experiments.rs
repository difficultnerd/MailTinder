//! T-902 experiments consent tests (S2 CL-02 AC2, AC3; S5 EXP-4; S7 5.13
//! API-EXP-1, API-EXP-2; ASVS V2.4.1, V16.3.3).
//!
//! The route tests run against the real router with the in-memory store, the
//! virtual clock and the recording rate limiter. The gate tests are unit tests
//! on `bakeoff_gate`.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::too_many_lines,
    clippy::similar_names,
    clippy::items_after_statements,
    clippy::needless_pass_by_value,
    clippy::missing_panics_doc
)]

use std::sync::Arc;

use api::experiments::{
    bakeoff_gate, consent_is_current, BakeoffGate, CURRENT_CONSENT_VERSION, OPT_CHANGES_PER_DAY,
};
use api::limits::policies;
use api::session::store::SessionService;
use api::state::AppState;
use api::{app_state, build_router, config::ApiConfig};
use axum::body::Body;
use axum::http::{header, HeaderValue, Method, Request, StatusCode};
use axum::response::Response;
use domain::{MessageClass, Provider, UserId};
use obs::{Pseudonymiser, Sensitive};
use ports::{
    AgeBucket, ClassifierEvalRecord, Clock, EvalHeaderFacts, EvalId, EvalOutcome,
    HeaderRulesResult, KeyService, Precondition, Rng, ServerStore, SwipeDirection,
    TextTokensBucket, UserPseudoId, UserRecord, WrappedKey,
};
use testkit::{fake_ports, Fakes};
use time::macros::datetime;
use time::{Duration, OffsetDateTime};
use tower::ServiceExt;
use uuid::Uuid;

type TestResult = Result<(), Box<dyn std::error::Error>>;

const ORIGIN: &str = "https://mailtinder.test";
const EMAIL_KEY: &[u8] = b"fake-email-key";
const LOG_KEY: &[u8] = b"fake-log-key";

fn config() -> Result<ApiConfig, Box<dyn std::error::Error>> {
    Ok(ApiConfig::new(
        ORIGIN.to_owned(),
        "fake-client".into(),
        Sensitive::new(LOG_KEY.to_vec()),
        Sensitive::new(EMAIL_KEY.to_vec()),
    )?)
}

fn fixture() -> Result<(AppState, Fakes), Box<dyn std::error::Error>> {
    let (ports, fakes) = fake_ports();
    Ok((app_state(Arc::new(ports), Arc::new(config()?)), fakes))
}

/// A time inside a test year.
fn now() -> OffsetDateTime {
    datetime!(2026-10-06 12:00:00 UTC)
}

/// A user record with the given consent fields.
fn user_record(version: Option<&str>, opted_in_at: Option<OffsetDateTime>) -> UserRecord {
    UserRecord {
        user_id: UserId::new(Uuid::from_u128(1)),
        created_at: now(),
        is_admin: false,
        wrapped_data_key: WrappedKey(vec![]),
        experiments_consent_version: version.map(str::to_owned),
        experiments_opted_in_at: opted_in_at,
    }
}

/// The pseudonym the API derives for `user` under the fixture's log key.
fn pseudo_of(user: &UserId) -> String {
    Pseudonymiser::new(Sensitive::new(LOG_KEY.to_vec()))
        .pseudo_id(&user.0)
        .as_str()
        .to_owned()
}

async fn seed_user(
    fakes: &Fakes,
    version: Option<&str>,
    opted_in_at: Option<OffsetDateTime>,
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
                is_admin: false,
                wrapped_data_key: wrapped,
                experiments_consent_version: version.map(str::to_owned),
                experiments_opted_in_at: opted_in_at,
            },
            Precondition::MustNotExist,
        )
        .await?;
    Ok(user)
}

/// Seed the `config/classifiers` document with the two switches.
async fn seed_switches(
    state: &AppState,
    gemini: bool,
    jev: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    state
        .ports
        .store
        .config()
        .put_classifiers(
            &ports::ClassifiersConfig {
                gemini_enabled: gemini,
                jev_enabled: jev,
                updated_at: now(),
            },
            Precondition::MustNotExist,
        )
        .await?;
    Ok(())
}

/// One `classifier_eval` record under `pseudo`, at index `n`.
async fn seed_eval(
    state: &AppState,
    pseudo: &str,
    n: u8,
) -> Result<EvalId, Box<dyn std::error::Error>> {
    let eval_id = EvalId(Uuid::from_u128(0x9000 + u128::from(n)));
    state
        .ports
        .store
        .classifier_eval()
        .put(
            &ClassifierEvalRecord {
                eval_id,
                user_pseudo_id: UserPseudoId(pseudo.to_owned()),
                created_at: now(),
                expires_at: now() + Duration::days(180),
                header_rules: HeaderRulesResult {
                    class: MessageClass::List,
                    score: 80,
                },
                gemini: None,
                jev: None,
                outcome: EvalOutcome {
                    direction: SwipeDirection::Left,
                    undone: false,
                    time_to_swipe_ms: 100,
                },
                header_facts: EvalHeaderFacts {
                    has_list_unsubscribe: true,
                    dkim_covers_list_unsubscribe: true,
                    has_list_unsubscribe_post: false,
                    has_list_id: false,
                    has_feedback_id: false,
                    precedence_bulk: false,
                    auto_submitted: false,
                    from_authenticated: true,
                },
                provider: Provider::Gmail,
                age_bucket: AgeBucket::Under7d,
                text_tokens_bucket: TextTokensBucket::Under100,
                lang_is_english: true,
                input_version: "v1".to_owned(),
                question_version: "v1".to_owned(),
                price_version: "v1".to_owned(),
            },
            Precondition::MustNotExist,
        )
        .await?;
    Ok(eval_id)
}

struct Authed {
    cookie: String,
    csrf: String,
}

async fn authed(
    state: &AppState,
    fakes: &Fakes,
    user: &UserId,
) -> Result<Authed, Box<dyn std::error::Error>> {
    let (cookie, record) = SessionService::new(state)
        .establish(None, user, Some(fakes.clock.now() - Duration::seconds(10)))
        .await?;
    let cookie = cookie
        .0
        .to_str()?
        .split(';')
        .next()
        .unwrap_or_default()
        .to_owned();
    Ok(Authed {
        cookie,
        csrf: record.record.csrf_token.clone(),
    })
}

fn get_req(cookie: &str) -> Result<Request<Body>, Box<dyn std::error::Error>> {
    Ok(Request::builder()
        .method(Method::GET)
        .uri("/api/v1/me/experiments")
        .header(header::COOKIE, HeaderValue::from_str(cookie)?)
        .body(Body::empty())?)
}

fn put_req(a: &Authed, body: &str) -> Result<Request<Body>, Box<dyn std::error::Error>> {
    Ok(Request::builder()
        .method(Method::PUT)
        .uri("/api/v1/me/experiments")
        .header(header::ORIGIN, ORIGIN)
        .header(header::COOKIE, a.cookie.as_str())
        .header("x-csrf-token", a.csrf.as_str())
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_owned()))?)
}

/// `{"classifier_bakeoff": {"opted_in": <opted>, "consent_version": <v>}}`.
fn put_body(opted_in: bool, version: Option<&str>) -> String {
    let v = version.map_or("null".to_owned(), |s| format!("\"{s}\""));
    format!("{{\"classifier_bakeoff\":{{\"opted_in\":{opted_in},\"consent_version\":{v}}}}}")
}

async fn json_of(resp: Response) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await?;
    Ok(serde_json::from_slice(&bytes)?)
}

async fn code_of(resp: Response) -> Result<String, Box<dyn std::error::Error>> {
    Ok(json_of(resp).await?["code"]
        .as_str()
        .unwrap_or_default()
        .to_owned())
}

async fn bakeoff_of(resp: Response) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    Ok(json_of(resp).await?["classifier_bakeoff"].clone())
}

// ---------------------------------------------------------------------------
// CL-02 AC2: the gate needs consent AND a switch
// ---------------------------------------------------------------------------

#[test]
fn cl_02_ac2_gate_closed_without_consent() {
    let user = user_record(None, None);
    let gate = bakeoff_gate(&user, (true, true));
    assert_eq!(
        gate,
        BakeoffGate {
            gemini: false,
            jev: false
        }
    );
}

#[test]
fn cl_02_ac2_gate_closed_for_outdated_version() {
    // A consent to an older text is opted out (S7 5.13).
    let user = user_record(Some("2025-01-01"), Some(now()));
    assert!(!consent_is_current(&user));
    assert_eq!(
        bakeoff_gate(&user, (true, true)),
        BakeoffGate {
            gemini: false,
            jev: false
        }
    );
}

#[test]
fn cl_02_ac2_gate_needs_switch_and_consent() {
    let consenting = user_record(Some(CURRENT_CONSENT_VERSION), Some(now()));
    // Both switches on: both open.
    assert_eq!(
        bakeoff_gate(&consenting, (true, true)),
        BakeoffGate {
            gemini: true,
            jev: true
        }
    );
    // Jev off: only Gemini.
    assert_eq!(
        bakeoff_gate(&consenting, (true, false)),
        BakeoffGate {
            gemini: true,
            jev: false
        }
    );
    // Gemini off: only Jev.
    assert_eq!(
        bakeoff_gate(&consenting, (false, true)),
        BakeoffGate {
            gemini: false,
            jev: true
        }
    );
    // Both off.
    assert_eq!(
        bakeoff_gate(&consenting, (false, false)),
        BakeoffGate {
            gemini: false,
            jev: false
        }
    );
    // No consent: closed whatever the switches say.
    let stranger = user_record(None, None);
    for switches in [(true, true), (true, false), (false, true), (false, false)] {
        assert_eq!(
            bakeoff_gate(&stranger, switches),
            BakeoffGate {
                gemini: false,
                jev: false
            }
        );
    }
}

// ---------------------------------------------------------------------------
// The route: PUT then GET
// ---------------------------------------------------------------------------

#[tokio::test]
async fn exp_4_opt_in_sets_and_opt_out_clears_consent_fields() -> TestResult {
    let (state, fakes) = fixture()?;
    seed_switches(&state, true, true).await?;
    let user = seed_user(&fakes, None, None).await?;
    let router = build_router(state.clone());
    let a = authed(&state, &fakes, &user).await?;

    // Opt in.
    let resp = router
        .clone()
        .oneshot(put_req(&a, &put_body(true, Some(CURRENT_CONSENT_VERSION)))?)
        .await?;
    assert_eq!(resp.status(), StatusCode::OK);
    let record = state
        .ports
        .store
        .users()
        .get(&user)
        .await?
        .ok_or("user")?
        .record;
    assert_eq!(
        record.experiments_consent_version.as_deref(),
        Some(CURRENT_CONSENT_VERSION)
    );
    assert!(
        record.experiments_opted_in_at.is_some(),
        "opt-in records the time"
    );
    assert!(consent_is_current(&record));

    let body = bakeoff_of(router.clone().oneshot(get_req(&a.cookie)?).await?).await?;
    assert_eq!(body["opted_in"], true);
    assert_eq!(body["consent_version"], CURRENT_CONSENT_VERSION);
    assert_eq!(body["current_consent_version"], CURRENT_CONSENT_VERSION);
    assert!(body["opted_in_at"].is_string());

    // Opt out clears both.
    let resp = router
        .clone()
        .oneshot(put_req(&a, &put_body(false, None))?)
        .await?;
    assert_eq!(resp.status(), StatusCode::OK);
    let record = state
        .ports
        .store
        .users()
        .get(&user)
        .await?
        .ok_or("user")?
        .record;
    assert!(record.experiments_consent_version.is_none());
    assert!(record.experiments_opted_in_at.is_none());
    let body = bakeoff_of(router.clone().oneshot(get_req(&a.cookie)?).await?).await?;
    assert_eq!(body["opted_in"], false);
    assert!(body["consent_version"].is_null());
    assert!(body["opted_in_at"].is_null());
    Ok(())
}

#[tokio::test]
async fn cl_02_ac3_opt_out_deletes_eval_records_before_response() -> TestResult {
    let (state, fakes) = fixture()?;
    seed_switches(&state, true, true).await?;
    let user = seed_user(&fakes, Some(CURRENT_CONSENT_VERSION), Some(now())).await?;
    let other = seed_user(&fakes, None, None).await?;
    let mine = pseudo_of(&user);
    let theirs = pseudo_of(&other);
    let mut ids = Vec::new();
    for n in 0..3 {
        ids.push(seed_eval(&state, &mine, n).await?);
    }
    let foreign = seed_eval(&state, &theirs, 9).await?;

    let router = build_router(state.clone());
    let a = authed(&state, &fakes, &user).await?;
    let resp = router
        .clone()
        .oneshot(put_req(&a, &put_body(false, None))?)
        .await?;
    assert_eq!(resp.status(), StatusCode::OK);

    for id in ids {
        assert!(
            state
                .ports
                .store
                .classifier_eval()
                .get(&id)
                .await?
                .is_none(),
            "the user's evaluation records are deleted before the response"
        );
    }
    assert!(
        state
            .ports
            .store
            .classifier_eval()
            .get(&foreign)
            .await?
            .is_some(),
        "another user's record is untouched"
    );
    Ok(())
}

#[tokio::test]
async fn cl_02_ac3_opt_out_closes_gate_immediately() -> TestResult {
    let (state, fakes) = fixture()?;
    seed_switches(&state, true, true).await?;
    let user = seed_user(&fakes, Some(CURRENT_CONSENT_VERSION), Some(now())).await?;
    let router = build_router(state.clone());
    let a = authed(&state, &fakes, &user).await?;

    // Open before.
    let record = state
        .ports
        .store
        .users()
        .get(&user)
        .await?
        .ok_or("user")?
        .record;
    assert_eq!(
        bakeoff_gate(&record, (true, true)),
        BakeoffGate {
            gemini: true,
            jev: true
        }
    );

    let resp = router
        .clone()
        .oneshot(put_req(&a, &put_body(false, None))?)
        .await?;
    assert_eq!(resp.status(), StatusCode::OK);

    // Closed on the very next read.
    let record = state
        .ports
        .store
        .users()
        .get(&user)
        .await?
        .ok_or("user")?
        .record;
    assert_eq!(
        bakeoff_gate(&record, (true, true)),
        BakeoffGate {
            gemini: false,
            jev: false
        }
    );
    Ok(())
}

#[tokio::test]
async fn cl_02_ac3_opt_out_works_while_paused() -> TestResult {
    let (state, fakes) = fixture()?;
    // No config document: both models off (the pilot is paused).
    let user = seed_user(&fakes, Some(CURRENT_CONSENT_VERSION), Some(now())).await?;
    let router = build_router(state.clone());
    let a = authed(&state, &fakes, &user).await?;

    let resp = router
        .clone()
        .oneshot(put_req(&a, &put_body(false, Some("2019-01-01")))?)
        .await?;
    assert_eq!(resp.status(), StatusCode::OK, "opting out always works");
    let record = state
        .ports
        .store
        .users()
        .get(&user)
        .await?
        .ok_or("user")?
        .record;
    assert!(record.experiments_consent_version.is_none());
    Ok(())
}

#[tokio::test]
async fn api_exp_2_old_consent_version_409() -> TestResult {
    let (state, fakes) = fixture()?;
    seed_switches(&state, true, true).await?;
    let user = seed_user(&fakes, None, None).await?;
    let router = build_router(state.clone());
    let a = authed(&state, &fakes, &user).await?;

    let resp = router
        .clone()
        .oneshot(put_req(&a, &put_body(true, Some("2024-01-01")))?)
        .await?;
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    assert_eq!(code_of(resp).await?, "consent_outdated");
    let record = state
        .ports
        .store
        .users()
        .get(&user)
        .await?
        .ok_or("user")?
        .record;
    assert!(
        record.experiments_consent_version.is_none(),
        "nothing written"
    );
    Ok(())
}

#[tokio::test]
async fn api_exp_2_opt_in_while_paused_409() -> TestResult {
    let (state, fakes) = fixture()?;
    // Both switches off.
    let user = seed_user(&fakes, None, None).await?;
    let router = build_router(state.clone());
    let a = authed(&state, &fakes, &user).await?;

    let resp = router
        .clone()
        .oneshot(put_req(&a, &put_body(true, Some(CURRENT_CONSENT_VERSION)))?)
        .await?;
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    assert_eq!(code_of(resp).await?, "experiment_unavailable");
    let record = state
        .ports
        .store
        .users()
        .get(&user)
        .await?
        .ok_or("user")?
        .record;
    assert!(
        record.experiments_consent_version.is_none(),
        "nothing written"
    );
    Ok(())
}

#[tokio::test]
async fn api_exp_1_available_false_when_both_off() -> TestResult {
    let (state, fakes) = fixture()?;
    let user = seed_user(&fakes, None, None).await?;
    let router = build_router(state.clone());
    let a = authed(&state, &fakes, &user).await?;

    let body = bakeoff_of(router.clone().oneshot(get_req(&a.cookie)?).await?).await?;
    assert_eq!(body["available"], false);

    // One model on is enough to make the experiment available.
    seed_switches(&state, false, true).await?;
    let body = bakeoff_of(router.clone().oneshot(get_req(&a.cookie)?).await?).await?;
    assert_eq!(body["available"], true);
    Ok(())
}

#[tokio::test]
async fn api_exp_1_outdated_version_reads_as_opted_out() -> TestResult {
    let (state, fakes) = fixture()?;
    seed_switches(&state, true, true).await?;
    // A consent to an older text.
    let user = seed_user(&fakes, Some("2020-01-01"), Some(now())).await?;
    let router = build_router(state.clone());
    let a = authed(&state, &fakes, &user).await?;

    let body = bakeoff_of(router.clone().oneshot(get_req(&a.cookie)?).await?).await?;
    assert_eq!(
        body["opted_in"], false,
        "an old version counts as opted out"
    );
    assert_eq!(
        body["consent_version"], "2020-01-01",
        "the stored version is reported verbatim"
    );
    Ok(())
}

#[tokio::test]
async fn asvs_v2_4_1_experiments_changes_rate_limited() -> TestResult {
    let (state, fakes) = fixture()?;
    seed_switches(&state, true, true).await?;
    let user = seed_user(&fakes, None, None).await?;
    let router = build_router(state.clone());
    let a = authed(&state, &fakes, &user).await?;

    assert_eq!(
        policies::EXPERIMENT_OPT.limit,
        OPT_CHANGES_PER_DAY,
        "the policy and the constant must agree"
    );

    for _ in 0..OPT_CHANGES_PER_DAY {
        let resp = router
            .clone()
            .oneshot(put_req(&a, &put_body(false, None))?)
            .await?;
        assert_eq!(resp.status(), StatusCode::OK);
    }
    let resp = router
        .clone()
        .oneshot(put_req(&a, &put_body(false, None))?)
        .await?;
    assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(code_of(resp).await?, "rate_limited");
    Ok(())
}

#[tokio::test]
async fn asvs_v16_3_3_consent_change_logged() -> TestResult {
    let (state, fakes) = fixture()?;
    seed_switches(&state, true, true).await?;
    let user = seed_user(&fakes, None, None).await?;
    let router = build_router(state.clone());
    let a = authed(&state, &fakes, &user).await?;

    let clock = obs::arc(obs::FixedClock(fakes.clock.now()));
    let (capture, _guard) = obs::capture("api", clock);

    let resp = router
        .clone()
        .oneshot(put_req(&a, &put_body(true, Some(CURRENT_CONSENT_VERSION)))?)
        .await?;
    assert_eq!(resp.status(), StatusCode::OK);
    let resp = router
        .clone()
        .oneshot(put_req(&a, &put_body(false, None))?)
        .await?;
    assert_eq!(resp.status(), StatusCode::OK);

    let text = capture.text();
    assert_eq!(
        text.matches("experiments_opt_in").count(),
        1,
        "one event per opt-in: {text}"
    );
    assert_eq!(
        text.matches("experiments_opt_out").count(),
        1,
        "one event per opt-out: {text}"
    );
    // Pseudonymous only: the raw user ID never reaches the log.
    assert!(
        !text.contains(&user.0.to_string()),
        "the log holds no raw user ID: {text}"
    );
    Ok(())
}

#[tokio::test]
async fn api_exp_2_unknown_field_400() -> TestResult {
    let (state, fakes) = fixture()?;
    seed_switches(&state, true, true).await?;
    let user = seed_user(&fakes, None, None).await?;
    let router = build_router(state.clone());
    let a = authed(&state, &fakes, &user).await?;

    let body =
        put_body(false, None).replace("\"opted_in\":false", "\"opted_in\":false,\"extra\":1");
    let resp = router.clone().oneshot(put_req(&a, &body)?).await?;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    assert_eq!(code_of(resp).await?, "invalid_request");
    Ok(())
}
