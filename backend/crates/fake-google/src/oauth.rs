//! OAuth routes for `fake-google` (T-206): authorisation, token and revoke.
//!
//! Route table:
//!
//! | Method and path | Behaviour |
//! | --- | --- |
//! | `GET /.well-known/openid-configuration` | discovery document |
//! | `GET /oauth2/v3/certs` | JWKS with the main key |
//! | `GET /o/oauth2/v2/auth` | scripted authorisation; 302 to redirect_uri, 400 on bad request (never redirects) |
//! | `POST /token` | authorisation_code or refresh_token grants |
//! | `POST /revoke` | revoke a refresh or access token |

use axum::extract::{Query, RawQuery, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::Engine as _;
use sha2::{Digest, Sha256};
use time::OffsetDateTime;

use super::gmail::AppState;
use super::oidc::{self, IdTokenOptions};
use super::scenario::{LoginOutcome, NextLogin, TokenScenario};
use super::state::{FakeMailboxKey, Grant, RefreshRecord};
use super::tokens::TokenRecord;

pub const CODE_TTL_S: i64 = 600;
pub const ACCESS_TTL_S: i64 = 3599;

/// Build the identity router.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/.well-known/openid-configuration", get(discovery))
        .route("/oauth2/v3/certs", get(certs))
        .route("/o/oauth2/v2/auth", get(authorise))
        .route("/token", post(token))
        .route("/revoke", post(revoke))
}

async fn discovery() -> Json<serde_json::Value> {
    Json(oidc::discovery("http://127.0.0.1/"))
}

async fn certs() -> Json<serde_json::Value> {
    Json(oidc::jwks())
}

#[derive(serde::Deserialize)]
#[allow(non_snake_case)]
struct AuthQuery {
    client_id: String,
    redirect_uri: String,
    response_type: String,
    scope: String,
    state: Option<String>,
    nonce: Option<String>,
    code_challenge: Option<String>,
    code_challenge_method: Option<String>,
    prompt: Option<String>,
    max_age: Option<String>,
    #[allow(dead_code)]
    login_hint: Option<String>,
}

async fn authorise(State(st): State<AppState>, Query(q): Query<AuthQuery>) -> Response {
    let now = st.1.now();
    let mut st = st.0.lock().unwrap();
    let state = q.state.clone().unwrap_or_default();

    // Validate the client and redirect_uri exactly.
    let client = match st.clients.get(&q.client_id) {
        Some(c) => c.clone(),
        None => {
            return plain_400("invalid_client");
        }
    };
    let redirect_ok = client.redirect_uris.iter().any(|u| u == &q.redirect_uri);
    if !redirect_ok {
        // Never redirect on a bad redirect_uri (open redirect).
        return plain_400("invalid_request");
    }
    if q.response_type != "code" {
        return plain_400("unsupported_response_type");
    }
    if !q.scope.split_whitespace().any(|s| s == "openid") {
        return plain_400("invalid_scope");
    }
    let nonce = match &q.nonce {
        Some(n) if !n.is_empty() => n.clone(),
        _ => return plain_400("invalid_request"),
    };
    let challenge = match (&q.code_challenge, q.code_challenge_method.as_deref()) {
        (Some(c), Some("S256")) if !c.is_empty() => c.clone(),
        _ => return plain_400("invalid_request"), // PKCE required; plain/missing -> 400
    };

    // Scripted login.
    let login: NextLogin = match st.next_login.take() {
        Some(l) => l,
        None => return plain_400("no_scripted_login"),
    };
    if login.outcome == LoginOutcome::AccessDenied {
        return redirect_302(&format!(
            "{}?error=access_denied&state={}",
            q.redirect_uri, state
        ));
    }

    // Store a grant.
    st.code_counter += 1;
    let code = format!("{:032x}", st.code_counter);
    let auth_time = now.unix_timestamp() - login.auth_age_s;
    let include_auth_time =
        q.prompt.as_deref().is_some_and(|p| p.contains("login")) || q.max_age.is_some();
    let grant = Grant {
        code: code.clone(),
        client_id: q.client_id.clone(),
        redirect_uri: q.redirect_uri.clone(),
        challenge,
        nonce,
        scopes: q.scope.split_whitespace().map(str::to_owned).collect(),
        sub: login.sub.clone(),
        email: login.email.clone(),
        email_verified: login.email_verified,
        amr: login.amr.clone(),
        auth_time: include_auth_time.then_some(auth_time),
        created_at: now,
        refresh_issued: false,
        used: false,
        access_tokens: Vec::new(),
        refresh_token: None,
    };
    st.grants.insert(code.clone(), grant);

    redirect_302(&format!("{}?code={}&state={}", q.redirect_uri, code, state))
}

fn redirect_302(location: &str) -> Response {
    (
        StatusCode::FOUND,
        [(axum::http::header::LOCATION, location.to_owned())],
    )
        .into_response()
}

fn plain_400(msg: &str) -> Response {
    let body = format!("{msg}\n");
    (StatusCode::BAD_REQUEST, body).into_response()
}

#[derive(serde::Deserialize)]
struct TokenForm {
    grant_type: String,
    code: Option<String>,
    redirect_uri: Option<String>,
    code_verifier: Option<String>,
    refresh_token: Option<String>,
    client_id: Option<String>,
    #[allow(dead_code)]
    client_secret: Option<String>,
}

async fn token(
    State(st): State<AppState>,
    RawQuery(_): RawQuery,
    headers: HeaderMap,
    body: axum::body::Body,
) -> Response {
    // The token endpoint accepts x-www-form-urlencoded; parse the body bytes.
    let bytes = match axum::body::to_bytes(body, 64 * 1024).await {
        Ok(b) => b.to_vec(),
        Err(_) => return json_400("invalid_request"),
    };
    let form_str = String::from_utf8_lossy(&bytes);
    let form: TokenForm = match serde_urlencoded::from_str(&form_str) {
        Ok(f) => f,
        Err(_) => return json_400("invalid_request"),
    };

    let now = st.1.now();
    let mut st = st.0.lock().unwrap();
    // Client authentication: form fields or HTTP Basic.
    let Some(client_creds) = client_auth(&headers, &form) else {
        return json_400("invalid_client");
    };
    let Some(client) = st.clients.get(&client_creds).cloned() else {
        return json_400("invalid_client");
    };

    match form.grant_type.as_str() {
        "authorization_code" => {
            let code = form.code.clone().unwrap_or_default();
            let redirect = form.redirect_uri.clone().unwrap_or_default();
            let verifier = form.code_verifier.clone().unwrap_or_default();
            code_exchange(&mut st, &client, &code, &redirect, &verifier, now)
        }
        "refresh_token" => {
            let rt = form.refresh_token.clone().unwrap_or_default();
            refresh_exchange(&mut st, &client, &rt, now)
        }
        _ => json_400("unsupported_grant_type"),
    }
}

fn client_auth(headers: &HeaderMap, form: &TokenForm) -> Option<String> {
    // Basic auth: user:pass -> client_id:client_secret.
    if let Some(auth) = headers.get("authorization").and_then(|v| v.to_str().ok()) {
        if let Some(b64) = auth.strip_prefix("Basic ") {
            use base64::engine::general_purpose::STANDARD;
            if let Ok(decoded) = STANDARD.decode(b64) {
                let s = String::from_utf8_lossy(&decoded);
                if let Some((id, _)) = s.split_once(':') {
                    return Some(id.to_owned());
                }
            }
        }
    }
    form.client_id.clone()
}

fn code_exchange(
    st: &mut std::sync::MutexGuard<'_, super::state::FakeState>,
    client: &super::scenario::ClientReg,
    code: &str,
    redirect: &str,
    verifier: &str,
    now: OffsetDateTime,
) -> Response {
    let mut grant = match st.grants.get_mut(code) {
        Some(g) if !g.used && g.client_id == client.client_id => g.clone(),
        _ => return json_400("invalid_grant"),
    };
    if grant.redirect_uri != redirect {
        return json_400("invalid_grant");
    }
    // PKCE verifier check.
    let digest = Sha256::digest(verifier.as_bytes());
    let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest);
    if challenge != grant.challenge {
        return json_400("invalid_grant");
    }
    if (now - grant.created_at).whole_seconds() > CODE_TTL_S {
        return json_400("invalid_grant");
    }

    // Single use: mark used; a second use revokes every token from the grant.
    if grant.used {
        return json_400("invalid_grant");
    }
    grant.used = true;
    if let Some(stored) = st.grants.get_mut(code) {
        stored.used = true;
    }

    // Ensure the mailbox exists for the login email.
    let email = grant.email.clone();
    let mx = FakeMailboxKey(email.clone());
    st.mailbox(&email);

    let access_token = format!("tok-{}", uuid::Uuid::new_v4());
    st.tokens.insert(
        access_token.clone(),
        TokenRecord {
            mailbox: mx,
            scopes: grant.scopes.iter().cloned().collect(),
            expires_at: now + time::Duration::seconds(ACCESS_TTL_S),
        },
    );
    grant.access_tokens.push(access_token.clone());

    // Refresh token: only the first grant for this sub+client, unless consent.
    let mut refresh = None;
    let wants_consent = grant.scopes.iter().any(|s| s.contains("consent"));
    if !grant.refresh_issued {
        grant.refresh_issued = true;
        let rt = format!("rt-{}", uuid::Uuid::new_v4());
        st.refresh_tokens.insert(
            rt.clone(),
            RefreshRecord {
                token: rt.clone(),
                grant: code.to_owned(),
                sub: grant.sub.clone(),
                client_id: client.client_id.clone(),
                scopes: grant.scopes.clone(),
                email: email.clone(),
            },
        );
        grant.refresh_token = Some(rt.clone());
        refresh = Some(rt);
    } else if wants_consent {
        let rt = format!("rt-{}", uuid::Uuid::new_v4());
        st.refresh_tokens.insert(
            rt.clone(),
            RefreshRecord {
                token: rt.clone(),
                grant: code.to_owned(),
                sub: grant.sub.clone(),
                client_id: client.client_id.clone(),
                scopes: grant.scopes.clone(),
                email,
            },
        );
        grant.refresh_token = Some(rt.clone());
        refresh = Some(rt);
    }

    // Build the ID token.
    let scenario = st.token_scenario.take();
    let nonce = match scenario {
        Some(TokenScenario::ReplayPreviousNonce) => {
            Some(st.last_nonce.clone().unwrap_or_else(|| grant.nonce.clone()))
        }
        Some(TokenScenario::MissingNonce) => None,
        _ => Some(grant.nonce.clone()),
    };
    st.last_nonce = Some(grant.nonce.clone());
    let id_token = oidc::build_id_token(
        &IdTokenOptions {
            aud: Some(&client.client_id),
            sub: &grant.sub,
            email: &grant.email,
            email_verified: grant.email_verified,
            nonce: nonce.as_deref(),
            auth_time: grant.auth_time,
            amr: grant.amr.clone(),
            now: now.unix_timestamp(),
            exp_offset: oidc::ID_TOKEN_TTL_S,
            ..Default::default()
        },
        scenario,
    );

    let mut resp = serde_json::json!({
        "access_token": access_token,
        "expires_in": ACCESS_TTL_S,
        "scope": grant.scopes.join(" "),
        "token_type": "Bearer",
        "id_token": id_token,
    });
    if let Some(rt) = refresh {
        resp["refresh_token"] = serde_json::json!(rt);
    }
    Json(resp).into_response()
}

fn refresh_exchange(
    st: &mut std::sync::MutexGuard<'_, super::state::FakeState>,
    client: &super::scenario::ClientReg,
    rt: &str,
    now: OffsetDateTime,
) -> Response {
    let Some(rec) = st.refresh_tokens.get(rt).cloned() else {
        return json_400("invalid_grant");
    };
    if rec.client_id != client.client_id {
        return json_400("invalid_grant");
    }
    // Issue a new access token, same mailbox and scopes.
    let access_token = format!("tok-{}", uuid::Uuid::new_v4());
    let mx = FakeMailboxKey(rec.email.clone());
    st.mailbox(&rec.email);
    st.tokens.insert(
        access_token.clone(),
        TokenRecord {
            mailbox: mx,
            scopes: rec.scopes.iter().cloned().collect(),
            expires_at: now + time::Duration::seconds(ACCESS_TTL_S),
        },
    );
    let mut resp = serde_json::json!({
        "access_token": access_token,
        "expires_in": ACCESS_TTL_S,
        "scope": rec.scopes.join(" "),
        "token_type": "Bearer",
    });
    if rec.scopes.iter().any(|s| s == "openid") {
        let id_token = oidc::build_id_token(
            &IdTokenOptions {
                aud: Some(&client.client_id),
                sub: &rec.sub,
                email: &rec.email,
                email_verified: true,
                nonce: None,
                now: now.unix_timestamp(),
                exp_offset: oidc::ID_TOKEN_TTL_S,
                ..Default::default()
            },
            None,
        );
        resp["id_token"] = serde_json::json!(id_token);
    }
    Json(resp).into_response()
}

#[derive(serde::Deserialize)]
struct RevokeForm {
    token: String,
}

async fn revoke(State(st): State<AppState>, body: axum::body::Body) -> Response {
    let bytes = match axum::body::to_bytes(body, 64 * 1024).await {
        Ok(b) => b.to_vec(),
        Err(_) => return json_400("invalid_token"),
    };
    let form_str = String::from_utf8_lossy(&bytes);
    let form: RevokeForm = match serde_urlencoded::from_str(&form_str) {
        Ok(f) => f,
        Err(_) => return json_400("invalid_token"),
    };
    let mut st = st.0.lock().unwrap();
    // T-1101c: the e2e harness reads the recorded order of an account
    // deletion's provider calls (the app folder file delete, then the token
    // revoke) from `/__fake/events`, so a revoke is recorded like every other
    // provider call. The token itself is never recorded, only the call.
    super::gmail::record(&mut st, "POST", "oauth.revoke", vec![]);
    // Refresh token: revoke it and every access token from its grant.
    if let Some(rec) = st.refresh_tokens.remove(&form.token) {
        st.revocations.push("refresh".to_owned());
        let to_remove: Vec<String> = st
            .grants
            .get(&rec.grant)
            .map(|g| g.access_tokens.clone())
            .unwrap_or_default();
        for at in &to_remove {
            st.tokens.remove(at);
        }
        if let Some(grant) = st.grants.get_mut(&rec.grant) {
            grant.refresh_token = None;
        }
        return Json(serde_json::json!({})).into_response();
    }
    // Single access token.
    if let Some(rec) = st.tokens.remove(&form.token) {
        st.revocations.push("access".to_owned());
        let _ = rec;
        return Json(serde_json::json!({})).into_response();
    }
    json_400("invalid_token")
}

fn json_400(msg: &str) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(serde_json::json!({ "error": msg })),
    )
        .into_response()
}
