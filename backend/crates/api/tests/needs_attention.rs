//! T-705 Needs Attention endpoint integration tests.
//!
//! S2 NA-01 AC1, AC2, UN-05 AC1; ASVS V8.2.2, V1.2.2 (API-NA-1 to API-NA-3).
//!
//! Everything runs against the real router with the in-memory store and keys
//! and the virtual clock. Records are written directly, since raising items is
//! T-701/T-704 (out of scope here).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::too_many_lines,
    clippy::similar_names,
    clippy::items_after_statements,
    clippy::needless_pass_by_value,
    clippy::missing_panics_doc
)]

use std::collections::BTreeSet;
use std::sync::Arc;

use api::sealed::{SealedTokens, TokenType};
use api::session::store::SessionService;
use api::state::AppState;
use api::{app_state, build_router, config::ApiConfig};
use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::response::Response;
use axum::Router;
use domain::{NeedsAttentionReason, UserId};
use obs::Sensitive;
use ports::store::aad_fields;
use ports::{
    Aad, Ciphertext, Clock, KeyService, NeedsAttentionId, NeedsAttentionRecord, Precondition, Rng,
    ServerStore, SessionRecordId,
};
use testkit::{fake_ports, Fakes};
use time::{Duration, OffsetDateTime};
use tower::ServiceExt;
use uuid::Uuid;

type TestResult = Result<(), Box<dyn std::error::Error>>;

const ORIGIN: &str = "https://mailtinder.test";
const EMAIL_KEY: &[u8] = b"fake-email-key";
/// The 30-day Needs Attention TTL (INV-2).
const RETAIN: Duration = Duration::days(30);

fn config() -> Result<ApiConfig, Box<dyn std::error::Error>> {
    Ok(ApiConfig::new(
        ORIGIN.to_owned(),
        "fake-client".into(),
        Sensitive::new(b"fake-log-key".to_vec()),
        Sensitive::new(EMAIL_KEY.to_vec()),
    )?)
}

fn fixture() -> Result<(AppState, Fakes), Box<dyn std::error::Error>> {
    let (ports, fakes) = fake_ports();
    Ok((app_state(Arc::new(ports), Arc::new(config()?)), fakes))
}

// ---------------------------------------------------------------------------
// Seed helpers
// ---------------------------------------------------------------------------

async fn seed_user(fakes: &Fakes) -> Result<UserId, Box<dyn std::error::Error>> {
    let user = UserId::new(fakes.rng.uuid_v4());
    let wrapped = fakes.keys.new_user_key(&user).await?;
    fakes
        .store
        .users()
        .put(
            &ports::UserRecord {
                user_id: user,
                created_at: fakes.clock.now(),
                is_admin: false,
                wrapped_data_key: wrapped,
                experiments_consent_version: None,
                experiments_opted_in_at: None,
            },
            Precondition::MustNotExist,
        )
        .await?;
    Ok(user)
}

async fn sealed_field(
    fakes: &Fakes,
    user: UserId,
    scope: &str,
    field: &'static str,
    value: &str,
) -> Result<Ciphertext, Box<dyn std::error::Error>> {
    let record = fakes.store.users().get(&user).await?.ok_or("seeded user")?;
    let aad = Aad {
        user,
        scope: scope.to_owned(),
        field,
    };
    Ok(Ciphertext(
        fakes
            .keys
            .seal(
                &user,
                &record.record.wrapped_data_key,
                &aad,
                value.as_bytes(),
            )
            .await?,
    ))
}

/// Write one item straight into the store (raising items is T-701/T-704).
async fn seed_item(
    fakes: &Fakes,
    user: UserId,
    sender_display: &str,
    link: Option<&str>,
    reason: NeedsAttentionReason,
    created_at: OffsetDateTime,
    expires_at: OffsetDateTime,
) -> Result<NeedsAttentionId, Box<dyn std::error::Error>> {
    let item_id = NeedsAttentionId(fakes.rng.uuid_v4());
    let scope = item_id.0.to_string();
    let sealed_sender = sealed_field(
        fakes,
        user,
        &scope,
        aad_fields::NA_SENDER_DISPLAY,
        sender_display,
    )
    .await?;
    let sealed_link = match link {
        Some(l) => Some(sealed_field(fakes, user, &scope, aad_fields::NA_LINK, l).await?),
        None => None,
    };
    fakes
        .store
        .needs_attention()
        .put(
            &NeedsAttentionRecord {
                item_id,
                user_id: user,
                mailbox_id: domain::MailboxId::new(fakes.rng.uuid_v4()),
                sender_display: sealed_sender,
                link: sealed_link,
                reason_code: reason,
                created_at,
                expires_at,
            },
            Precondition::MustNotExist,
        )
        .await?;
    Ok(item_id)
}

// ---------------------------------------------------------------------------
// Request helpers
// ---------------------------------------------------------------------------

struct Authed {
    cookie: String,
    csrf: String,
    user: UserId,
    session_record_id: SessionRecordId,
}

async fn authed(state: &AppState, fakes: &Fakes) -> Result<Authed, Box<dyn std::error::Error>> {
    let user = seed_user(fakes).await?;
    let (cookie, record) = SessionService::new(state)
        .establish(None, &user, Some(fakes.clock.now() - Duration::seconds(10)))
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
        user,
        session_record_id: record.record.session_record_id,
    })
}

async fn body_string(resp: Response) -> Result<String, Box<dyn std::error::Error>> {
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

async fn get(
    router: &Router,
    cookie: &str,
    uri: &str,
) -> Result<Response, Box<dyn std::error::Error>> {
    let req = Request::builder()
        .method(Method::GET)
        .uri(uri)
        .header(header::COOKIE, cookie)
        .body(Body::empty())?;
    Ok(router.clone().oneshot(req).await?)
}

async fn list_json(
    router: &Router,
    cookie: &str,
    query: &str,
) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    let uri = if query.is_empty() {
        "/api/v1/needs-attention".to_owned()
    } else {
        format!("/api/v1/needs-attention?{query}")
    };
    let resp = get(router, cookie, &uri).await?;
    assert_eq!(resp.status(), StatusCode::OK, "GET {uri}");
    Ok(serde_json::from_str(&body_string(resp).await?)?)
}

async fn post_item(
    router: &Router,
    a: &Authed,
    item_id: Uuid,
    action: &str,
) -> Result<Response, Box<dyn std::error::Error>> {
    let req = Request::builder()
        .method(Method::POST)
        .uri(format!("/api/v1/needs-attention/{item_id}/{action}"))
        .header(header::ORIGIN, ORIGIN)
        .header(header::COOKIE, a.cookie.as_str())
        .header("x-csrf-token", a.csrf.as_str())
        .body(Body::empty())?;
    Ok(router.clone().oneshot(req).await?)
}

fn item_ids(json: &serde_json::Value) -> Vec<String> {
    json["items"]
        .as_array()
        .expect("items array")
        .iter()
        .map(|i| i["item_id"].as_str().unwrap_or_default().to_owned())
        .collect()
}

// ---------------------------------------------------------------------------
// NA-01 AC1: the open list, newest first, with sender, reason and link.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn na_01_ac1_lists_open_items_newest_first() -> TestResult {
    let (state, fakes) = fixture()?;
    let router = build_router(state.clone());
    let a = authed(&state, &fakes).await?;
    let now = fakes.clock.now();
    let oldest = seed_item(
        &fakes,
        a.user,
        "Oldest",
        None,
        NeedsAttentionReason::JobExpired,
        now - Duration::minutes(2),
        now + RETAIN,
    )
    .await?;
    let middle = seed_item(
        &fakes,
        a.user,
        "Middle",
        None,
        NeedsAttentionReason::UnsubscribeIgnored,
        now - Duration::minutes(1),
        now + RETAIN,
    )
    .await?;
    let newest = seed_item(
        &fakes,
        a.user,
        "Newest",
        None,
        NeedsAttentionReason::UnsubscribeFailed,
        now,
        now + RETAIN,
    )
    .await?;

    let json = list_json(&router, &a.cookie, "").await?;
    assert_eq!(
        item_ids(&json),
        vec![
            newest.0.to_string(),
            middle.0.to_string(),
            oldest.0.to_string()
        ],
        "newest first"
    );
    Ok(())
}

#[tokio::test]
async fn na_01_ac1_item_has_sender_reason_link() -> TestResult {
    let (state, fakes) = fixture()?;
    let router = build_router(state.clone());
    let a = authed(&state, &fakes).await?;
    let now = fakes.clock.now();
    seed_item(
        &fakes,
        a.user,
        "Newsletter <news@example.com>",
        Some("https://example.com/unsubscribe?id=1"),
        NeedsAttentionReason::HttpsOnlyUnsubscribe,
        now,
        now + RETAIN,
    )
    .await?;

    let json = list_json(&router, &a.cookie, "").await?;
    let first = &json["items"][0];
    assert_eq!(first["sender_display"], "Newsletter <news@example.com>");
    assert_eq!(first["reason_code"], "https_only_unsubscribe");
    assert_eq!(first["link"], "https://example.com/unsubscribe?id=1");
    Ok(())
}

#[tokio::test]
async fn na_01_ac1_open_count_matches_items() -> TestResult {
    let (state, fakes) = fixture()?;
    let router = build_router(state.clone());
    let a = authed(&state, &fakes).await?;
    let b = authed(&state, &fakes).await?;
    let now = fakes.clock.now();
    for n in 0..3 {
        seed_item(
            &fakes,
            a.user,
            "Sender",
            None,
            NeedsAttentionReason::UnsubscribeFailed,
            now + Duration::seconds(n),
            now + RETAIN,
        )
        .await?;
    }
    // Another user's item never counts towards the caller's badge.
    seed_item(
        &fakes,
        b.user,
        "Not mine",
        None,
        NeedsAttentionReason::UnsubscribeFailed,
        now,
        now + RETAIN,
    )
    .await?;

    let json = list_json(&router, &a.cookie, "").await?;
    assert_eq!(json["open_count"], 3);
    assert_eq!(json["items"].as_array().expect("items").len(), 3);
    Ok(())
}

#[tokio::test]
async fn na_01_ac1_pages_with_sealed_cursor() -> TestResult {
    let (state, fakes) = fixture()?;
    let router = build_router(state.clone());
    let a = authed(&state, &fakes).await?;
    let now = fakes.clock.now();
    for n in 0..25 {
        seed_item(
            &fakes,
            a.user,
            "Sender",
            None,
            NeedsAttentionReason::UnsubscribeFailed,
            now + Duration::seconds(n),
            now + RETAIN,
        )
        .await?;
    }

    let first = list_json(&router, &a.cookie, "limit=20").await?;
    assert_eq!(first["items"].as_array().expect("items").len(), 20);
    let cursor = first["next_cursor"]
        .as_str()
        .expect("a next cursor")
        .to_owned();

    let second = list_json(&router, &a.cookie, &format!("limit=20&cursor={cursor}")).await?;
    assert_eq!(second["items"].as_array().expect("items").len(), 5);
    assert!(second["next_cursor"].is_null(), "the end has no cursor");

    let mut seen = BTreeSet::new();
    for id in item_ids(&first).into_iter().chain(item_ids(&second)) {
        assert!(seen.insert(id), "no item appears twice");
    }
    assert_eq!(seen.len(), 25);
    Ok(())
}

// ---------------------------------------------------------------------------
// NA-01 AC2: resolve and dismiss delete; expired items are not listed.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn na_01_ac2_resolve_deletes_item() -> TestResult {
    let (state, fakes) = fixture()?;
    let router = build_router(state.clone());
    let a = authed(&state, &fakes).await?;
    let now = fakes.clock.now();
    let item = seed_item(
        &fakes,
        a.user,
        "Sender",
        Some("https://example.com/u"),
        NeedsAttentionReason::HttpsOnlyUnsubscribe,
        now,
        now + RETAIN,
    )
    .await?;

    let resp = post_item(&router, &a, item.0, "resolve").await?;
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    assert!(
        fakes.store.needs_attention().get(&item).await?.is_none(),
        "the item is deleted"
    );
    let json = list_json(&router, &a.cookie, "").await?;
    assert!(item_ids(&json).is_empty(), "the list is empty again");
    Ok(())
}

#[tokio::test]
async fn na_01_ac2_dismiss_deletes_item() -> TestResult {
    let (state, fakes) = fixture()?;
    let router = build_router(state.clone());
    let a = authed(&state, &fakes).await?;
    let now = fakes.clock.now();
    let item = seed_item(
        &fakes,
        a.user,
        "Sender",
        None,
        NeedsAttentionReason::UnsubscribeIgnored,
        now,
        now + RETAIN,
    )
    .await?;

    let resp = post_item(&router, &a, item.0, "dismiss").await?;
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    assert!(
        fakes.store.needs_attention().get(&item).await?.is_none(),
        "the item is deleted"
    );
    Ok(())
}

#[tokio::test]
async fn na_01_ac2_expired_item_not_listed() -> TestResult {
    let (state, fakes) = fixture()?;
    let router = build_router(state.clone());
    let a = authed(&state, &fakes).await?;
    let now = fakes.clock.now();
    // Expired a minute ago; the sweeper has not run yet.
    let expired = seed_item(
        &fakes,
        a.user,
        "Expired",
        None,
        NeedsAttentionReason::JobExpired,
        now - Duration::days(30) - Duration::minutes(1),
        now - Duration::minutes(1),
    )
    .await?;
    let live = seed_item(
        &fakes,
        a.user,
        "Live",
        None,
        NeedsAttentionReason::UnsubscribeFailed,
        now,
        now + RETAIN,
    )
    .await?;

    let json = list_json(&router, &a.cookie, "").await?;
    let ids = item_ids(&json);
    assert_eq!(ids, vec![live.0.to_string()]);
    assert!(!ids.contains(&expired.0.to_string()));
    Ok(())
}

// ---------------------------------------------------------------------------
// UN-05 AC1: the item carries the sender, the link and the reason.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn un_05_ac1_item_fields_returned() -> TestResult {
    let (state, fakes) = fixture()?;
    let router = build_router(state.clone());
    let a = authed(&state, &fakes).await?;
    let now = fakes.clock.now();
    let item = seed_item(
        &fakes,
        a.user,
        "Weekly Digest",
        Some("https://lists.example.com/unsub?t=abc"),
        NeedsAttentionReason::HttpsOnlyUnsubscribe,
        now,
        now + RETAIN,
    )
    .await?;

    let json = list_json(&router, &a.cookie, "").await?;
    let first = &json["items"][0];
    assert_eq!(first["item_id"], item.0.to_string());
    assert_eq!(first["sender_display"], "Weekly Digest");
    assert_eq!(first["link"], "https://lists.example.com/unsub?t=abc");
    assert_eq!(first["reason_code"], "https_only_unsubscribe");
    assert!(first["mailbox_id"].is_string());
    assert!(first["created_at"].is_string());
    Ok(())
}

// ---------------------------------------------------------------------------
// ASVS V8.2.2: another user's item is 404, never 403, and it survives.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn asvs_v8_2_2_other_users_item_returns_404() -> TestResult {
    let (state, fakes) = fixture()?;
    let router = build_router(state.clone());
    let a = authed(&state, &fakes).await?;
    let b = authed(&state, &fakes).await?;
    let now = fakes.clock.now();
    let item = seed_item(
        &fakes,
        a.user,
        "A's sender",
        None,
        NeedsAttentionReason::UnsubscribeFailed,
        now,
        now + RETAIN,
    )
    .await?;

    for action in ["resolve", "dismiss"] {
        let resp = post_item(&router, &b, item.0, action).await?;
        assert_eq!(
            resp.status(),
            StatusCode::NOT_FOUND,
            "{action} on another user's item hides it"
        );
    }
    assert!(
        fakes.store.needs_attention().get(&item).await?.is_some(),
        "the item is untouched"
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// ASVS V16.3.2: the refusal on another user's item is logged (security-review
// F1). The event names the action and carries no item ID.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn asvs_v16_3_2_other_users_item_refusal_logged() -> TestResult {
    let (state, fakes) = fixture()?;
    let router = build_router(state.clone());
    let a = authed(&state, &fakes).await?;
    let b = authed(&state, &fakes).await?;
    let now = fakes.clock.now();
    let item = seed_item(
        &fakes,
        a.user,
        "A's sender",
        None,
        NeedsAttentionReason::UnsubscribeFailed,
        now,
        now + RETAIN,
    )
    .await?;

    let clock = obs::arc(obs::FixedClock(fakes.clock.now()));
    let (capture, _guard) = obs::capture("api", clock);

    let resp = post_item(&router, &b, item.0, "dismiss").await?;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    let text = capture.text();
    assert!(
        text.contains("authz_failure"),
        "a foreign-item refusal is logged as authz_failure: {text}"
    );
    assert!(
        !text.contains(&item.0.to_string()),
        "the log line carries no item ID: {text}"
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// ASVS V1.2.2: only https links leave the server.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn asvs_v1_2_2_non_https_link_never_returned() -> TestResult {
    let (state, fakes) = fixture()?;
    let router = build_router(state.clone());
    let a = authed(&state, &fakes).await?;
    let now = fakes.clock.now();
    // Written straight into the store, bypassing the raising path.
    let http_item = seed_item(
        &fakes,
        a.user,
        "Plain http",
        Some("http://example.com/unsub"),
        NeedsAttentionReason::HttpsOnlyUnsubscribe,
        now,
        now + RETAIN,
    )
    .await?;
    let https_item = seed_item(
        &fakes,
        a.user,
        "Secure",
        Some("https://example.com/unsub"),
        NeedsAttentionReason::HttpsOnlyUnsubscribe,
        now + Duration::seconds(1),
        now + RETAIN,
    )
    .await?;

    let json = list_json(&router, &a.cookie, "").await?;
    for item in json["items"].as_array().expect("items") {
        let id = item["item_id"].as_str().unwrap_or_default();
        if id == http_item.0.to_string() {
            assert!(item["link"].is_null(), "an http link is never returned");
        }
        if id == https_item.0.to_string() {
            assert_eq!(item["link"], "https://example.com/unsub");
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// API-NA-1: a bad cursor is 400, never a 500 or an empty page.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn api_na_1_tampered_cursor_400() -> TestResult {
    let (state, fakes) = fixture()?;
    let router = build_router(state.clone());
    let a = authed(&state, &fakes).await?;
    let now = fakes.clock.now();
    for n in 0..25 {
        seed_item(
            &fakes,
            a.user,
            "Sender",
            None,
            NeedsAttentionReason::UnsubscribeFailed,
            now + Duration::seconds(n),
            now + RETAIN,
        )
        .await?;
    }
    let first = list_json(&router, &a.cookie, "limit=20").await?;
    let cursor = first["next_cursor"]
        .as_str()
        .expect("a next cursor")
        .to_owned();
    let mut tampered = cursor.clone().into_bytes();
    let last = tampered.len() - 1;
    tampered[last] = if tampered[last] == b'A' { b'B' } else { b'A' };
    let tampered = String::from_utf8(tampered)?;

    let resp = get(
        &router,
        &a.cookie,
        &format!("/api/v1/needs-attention?limit=20&cursor={tampered}"),
    )
    .await?;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    Ok(())
}

#[tokio::test]
async fn api_na_1_undo_token_as_cursor_400() -> TestResult {
    let (state, fakes) = fixture()?;
    let router = build_router(state.clone());
    let a = authed(&state, &fakes).await?;
    let record = fakes
        .store
        .users()
        .get(&a.user)
        .await?
        .ok_or("seeded user")?;
    // A well-formed token of the wrong type must not be accepted as a cursor
    // (S7 2.1, ASVS V9.2.2).
    let undo = SealedTokens::new(fakes.keys.clone(), fakes.clock.clone())
        .seal(
            TokenType::Undo,
            &a.user,
            &record.record.wrapped_data_key,
            &a.session_record_id,
            fakes.clock.now() + Duration::hours(1),
            &"undo-payload",
        )
        .await?;

    let resp = get(
        &router,
        &a.cookie,
        &format!("/api/v1/needs-attention?cursor={undo}"),
    )
    .await?;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    Ok(())
}

// ---------------------------------------------------------------------------
// API-NA-1: the query string is strict (unknown parameter is 400).
// ---------------------------------------------------------------------------

#[tokio::test]
async fn api_na_1_unknown_query_parameter_400() -> TestResult {
    let (state, fakes) = fixture()?;
    let router = build_router(state.clone());
    let a = authed(&state, &fakes).await?;

    let resp = get(&router, &a.cookie, "/api/v1/needs-attention?status=all").await?;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let resp = get(&router, &a.cookie, "/api/v1/needs-attention?limit=0").await?;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let resp = get(&router, &a.cookie, "/api/v1/needs-attention?limit=51").await?;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    Ok(())
}
