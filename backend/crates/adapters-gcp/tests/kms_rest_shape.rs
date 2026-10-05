//! T-302 `CloudKms` REST shape tests against a local `axum` stub, wired
//! through `GcpHttp::with_emulator`.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use adapters_gcp::{CloudKms, GcpHttp, KmsApi, StaticTokenSource, TokenSource};
use base64::Engine;
use obs::Sensitive;
use ports::KeyError;

const KEY_NAME: &str = "projects/p/locations/us-central1/keyRings/r/cryptoKeys/k";

/// A stub Cloud KMS that records the last request body and returns a fixed
/// ciphertext with correct CRC32C verification fields.
async fn stub_server() -> (SocketAddr, Arc<Mutex<Option<serde_json::Value>>>) {
    let recorded: Arc<Mutex<Option<serde_json::Value>>> = Arc::new(Mutex::new(None));
    let recorded_enc = Arc::clone(&recorded);
    let recorded_dec = Arc::clone(&recorded);

    let app = axum::Router::new()
        .route(
            &format!("/v1/{KEY_NAME}:encrypt"),
            axum::routing::post(move |body: axum::Json<serde_json::Value>| {
                let recorded = Arc::clone(&recorded_enc);
                async move {
                    *recorded.lock().unwrap() = Some(body.0.clone());
                    let ciphertext = b"kms-ciphertext-bytes";
                    axum::Json(serde_json::json!({
                        "name": KEY_NAME,
                        "ciphertext": base64::engine::general_purpose::STANDARD.encode(ciphertext),
                        "ciphertextCrc32c": crc32c::crc32c(ciphertext).to_string(),
                        "verifiedPlaintextCrc32c": true,
                        "verifiedAdditionalAuthenticatedDataCrc32c": true,
                    }))
                }
            }),
        )
        .route(
            &format!("/v1/{KEY_NAME}:decrypt"),
            axum::routing::post(move |body: axum::Json<serde_json::Value>| {
                let recorded = Arc::clone(&recorded_dec);
                async move {
                    *recorded.lock().unwrap() = Some(body.0.clone());
                    let plaintext = b"unwrapped-dek";
                    axum::Json(serde_json::json!({
                        "plaintext": base64::engine::general_purpose::STANDARD.encode(plaintext),
                        "plaintextCrc32c": crc32c::crc32c(plaintext).to_string(),
                        "usedPrimary": true,
                    }))
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
    (addr, recorded)
}

fn client(addr: SocketAddr) -> CloudKms {
    let tokens: Arc<dyn TokenSource> = Arc::new(StaticTokenSource(Sensitive::new("t".to_owned())));
    let http = Arc::new(GcpHttp::with_emulator(tokens, addr).expect("emulator client"));
    CloudKms::new(http, KEY_NAME.to_owned())
}

#[tokio::test]
async fn kms_rest_encrypt_sends_base64_and_crc32c_and_checks_verification() -> Result<(), String> {
    let (addr, recorded) = stub_server().await;
    let kms = client(addr);
    let plaintext = b"the-data-key";
    let aad = b"dek-aad";
    let ct = kms
        .encrypt(plaintext, aad)
        .await
        .map_err(|e| format!("{e:?}"))?;
    assert_eq!(ct, b"kms-ciphertext-bytes");

    // The request body carried base64 plaintext/AAD and decimal CRC32C fields.
    let body = recorded.lock().unwrap().clone().expect("request recorded");
    assert_eq!(
        body["plaintext"],
        serde_json::Value::String(base64::engine::general_purpose::STANDARD.encode(plaintext))
    );
    assert_eq!(
        body["additionalAuthenticatedData"],
        serde_json::Value::String(base64::engine::general_purpose::STANDARD.encode(aad))
    );
    assert_eq!(
        body["plaintextCrc32c"],
        serde_json::Value::String(crc32c::crc32c(plaintext).to_string())
    );
    assert_eq!(
        body["additionalAuthenticatedDataCrc32c"],
        serde_json::Value::String(crc32c::crc32c(aad).to_string())
    );
    Ok(())
}

#[tokio::test]
async fn kms_rest_decrypt_maps_400_to_open_failed() -> Result<(), String> {
    // A stub that returns 400 INVALID_ARGUMENT on decrypt.
    let app = axum::Router::new().route(
        &format!("/v1/{KEY_NAME}:decrypt"),
        axum::routing::post(|| async {
            (
                axum::http::StatusCode::BAD_REQUEST,
                axum::Json(serde_json::json!({
                    "error": {
                        "code": 400,
                        "status": "INVALID_ARGUMENT",
                        "message": "decryption failed",
                    }
                })),
            )
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    let kms = client(addr);
    let err = kms
        .decrypt(b"ciphertext", b"aad")
        .await
        .expect_err("must map 400 to OpenFailed");
    assert_eq!(err, KeyError::OpenFailed);
    Ok(())
}
