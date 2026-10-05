//! T-305: the `SecretManagerSecrets` request shape and error mapping against
//! a local `axum` stub of the Secret Manager REST API, wired through
//! `GcpHttp::with_emulator`.

#![cfg(feature = "test-support")]
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use adapters_gcp::{GcpHttp, SecretManagerSecrets, SecretsConfig, StaticTokenSource, TokenSource};
use base64::Engine;
use obs::Sensitive;
use ports::{SecretError, SecretName, Secrets};
use url::Url;

const PROJECT: &str = "p";

/// A stub Secret Manager API that serves a fixed payload per secret and
/// counts requests. Interior mutability lets tests arm a status/payload on a
/// shared `Arc`.
struct Stub {
    requests: AtomicUsize,
    status: Mutex<axum::http::StatusCode>,
    payload: Mutex<Option<(Vec<u8>, Option<u32>)>>,
}

impl Stub {
    fn new() -> Self {
        Self {
            requests: AtomicUsize::new(0),
            status: Mutex::new(axum::http::StatusCode::OK),
            payload: Mutex::new(None),
        }
    }

    fn count(&self) -> usize {
        self.requests.load(Ordering::SeqCst)
    }

    fn set_status(&self, status: axum::http::StatusCode) {
        *self.status.lock().expect("status lock") = status;
    }

    fn set_payload(&self, payload: Option<(Vec<u8>, Option<u32>)>) {
        *self.payload.lock().expect("payload lock") = payload;
    }
}

async fn stub_server(stub: Arc<Stub>) -> SocketAddr {
    let app = axum::Router::new().route(
        "/v1/{*rest}",
        axum::routing::get(move |uri: axum::http::Uri| {
            let stub = Arc::clone(&stub);
            async move {
                stub.requests.fetch_add(1, Ordering::SeqCst);
                let status = *stub.status.lock().expect("status lock");
                if status != axum::http::StatusCode::OK {
                    let code = match status {
                        axum::http::StatusCode::NOT_FOUND => "NOT_FOUND",
                        axum::http::StatusCode::FORBIDDEN => "PERMISSION_DENIED",
                        _ => "UNAVAILABLE",
                    };
                    return (
                        status,
                        axum::Json(serde_json::json!({
                            "error": { "code": status.as_u16(), "status": code, "message": "x" }
                        })),
                    );
                }
                let (data, crc) = stub
                    .payload
                    .lock()
                    .expect("payload lock")
                    .clone()
                    .expect("payload set");
                let mut payload = serde_json::json!({
                    "data": base64::engine::general_purpose::STANDARD.encode(&data),
                });
                if let Some(c) = crc {
                    payload["dataCrc32c"] = serde_json::Value::String(c.to_string());
                }
                (
                    axum::http::StatusCode::OK,
                    axum::Json(serde_json::json!({ "name": uri.path(), "payload": payload })),
                )
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    addr
}

fn client(addr: SocketAddr) -> SecretManagerSecrets {
    let tokens: Arc<dyn TokenSource> = Arc::new(StaticTokenSource(Sensitive::new("t".to_owned())));
    let http = Arc::new(GcpHttp::with_emulator(tokens, addr).expect("emulator client"));
    let base = Url::parse(&format!("http://{addr}/v1/")).expect("url");
    SecretManagerSecrets::with_base(
        http,
        SecretsConfig {
            project_id: PROJECT.to_owned(),
        },
        base,
    )
}

/// The access request decodes the base64 payload and checks the CRC32C.
#[tokio::test]
async fn secrets_access_decodes_and_checks_crc32c() -> Result<(), String> {
    let stub = Arc::new(Stub::new());
    let data = b"super-secret-value";
    stub.set_payload(Some((data.to_vec(), Some(crc32c::crc32c(data)))));
    let addr = stub_server(Arc::clone(&stub)).await;
    let s = client(addr);
    let got = s
        .get(SecretName::JevApiKey)
        .await
        .map_err(|e| format!("{e:?}"))?;
    if got.expose() != data {
        return Err("decoded value mismatch".into());
    }
    Ok(())
}

/// A CRC mismatch is `Unavailable`.
#[tokio::test]
async fn secrets_crc_mismatch_is_unavailable() -> Result<(), String> {
    let stub = Arc::new(Stub::new());
    stub.set_payload(Some((b"value".to_vec(), Some(0))));
    let addr = stub_server(Arc::clone(&stub)).await;
    let s = client(addr);
    let err = s
        .get(SecretName::JevApiKey)
        .await
        .err()
        .ok_or_else(|| "should fail".to_owned())?;
    if !matches!(err, SecretError::Unavailable) {
        return Err(format!("expected Unavailable, got {err:?}"));
    }
    Ok(())
}

/// A second read of the same secret is served from cache: one request for two
/// `get`s.
#[tokio::test]
async fn secrets_cached_after_first_read() -> Result<(), String> {
    let stub = Arc::new(Stub::new());
    stub.set_payload(Some((b"value".to_vec(), None)));
    let addr = stub_server(Arc::clone(&stub)).await;
    let s = client(addr);
    s.get(SecretName::JevApiKey)
        .await
        .map_err(|e| format!("{e:?}"))?;
    s.get(SecretName::JevApiKey)
        .await
        .map_err(|e| format!("{e:?}"))?;
    if stub.count() != 1 {
        return Err(format!("expected 1 request, got {}", stub.count()));
    }
    Ok(())
}

/// 404 maps to `Missing`, 403 to `Denied`.
#[tokio::test]
async fn secrets_404_is_missing_403_is_denied() -> Result<(), String> {
    // 404 -> Missing
    {
        let stub = Arc::new(Stub::new());
        stub.set_status(axum::http::StatusCode::NOT_FOUND);
        let addr = stub_server(Arc::clone(&stub)).await;
        let s = client(addr);
        let err = s
            .get(SecretName::JevApiKey)
            .await
            .err()
            .ok_or_else(|| "should fail".to_owned())?;
        if !matches!(err, SecretError::Missing) {
            return Err(format!("expected Missing, got {err:?}"));
        }
    }
    // 403 -> Denied
    {
        let stub = Arc::new(Stub::new());
        stub.set_status(axum::http::StatusCode::FORBIDDEN);
        let addr = stub_server(Arc::clone(&stub)).await;
        let s = client(addr);
        let err = s
            .get(SecretName::JevApiKey)
            .await
            .err()
            .ok_or_else(|| "should fail".to_owned())?;
        if !matches!(err, SecretError::Denied) {
            return Err(format!("expected Denied, got {err:?}"));
        }
    }
    Ok(())
}

/// `preload` fails fast when a secret is missing.
#[tokio::test]
async fn secrets_preload_fails_fast_on_missing() -> Result<(), String> {
    let stub = Arc::new(Stub::new());
    stub.set_status(axum::http::StatusCode::NOT_FOUND);
    let addr = stub_server(Arc::clone(&stub)).await;
    let s = client(addr);
    let err = s
        .preload(&[SecretName::JevApiKey])
        .await
        .err()
        .ok_or_else(|| "preload should fail".to_owned())?;
    if !matches!(err, SecretError::Missing) {
        return Err(format!("expected Missing, got {err:?}"));
    }
    Ok(())
}
