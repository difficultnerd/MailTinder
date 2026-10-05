//! The shared Gmail HTTP client: base URL, bearer auth, JSON decode.
//!
//! Every call goes through [`HttpEgress`] so the per-service allowlist holds
//! (S10 7.2). The adapter never retries and never sleeps; a rate limit comes
//! back as [`MailError::RateLimited`] for the caller to handle.

use std::sync::Arc;
use std::time::Duration;

use obs::{op_log, OpLog, Sensitive};
use ports::{Clock, EgressError, EgressRequest, HttpEgress, HttpMethod, MailError, MailboxCtx};
use serde::de::DeserializeOwned;
use serde::Serialize;
use url::Url;

use crate::errors::map_gmail_error;

/// The production base URL, the only host this adapter may reach.
#[allow(dead_code)] // production wiring (T-503, T-602) uses it; tests point at fake-google.
pub const GMAIL_BASE: &str = "https://gmail.googleapis.com/gmail/v1/users/me";
/// The per-call timeout, matching the one-click timeout in S6 6 `[DEFAULT]`.
pub const GMAIL_TIMEOUT_S: u64 = 10;
/// Bodies larger than this are refused as transient `[DEFAULT]`.
pub const MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;

/// The shared HTTP client for the Gmail REST API.
pub struct GmailHttp {
    egress: Arc<dyn HttpEgress>,
    base: Url,
    clock: Arc<dyn Clock>,
}

impl GmailHttp {
    /// Build a client. `base` is [`GMAIL_BASE`] in production, the fake-google
    /// base in tests.
    pub fn new(egress: Arc<dyn HttpEgress>, base: Url, clock: Arc<dyn Clock>) -> Self {
        Self {
            egress,
            base,
            clock,
        }
    }

    /// `GET` `path` (relative to the base) and decode the JSON body.
    pub async fn get_json<T: DeserializeOwned>(
        &self,
        mb: &MailboxCtx,
        path: &str,
        query: &[(&str, String)],
    ) -> Result<T, MailError> {
        self.send(mb, HttpMethod::Get, path, query, None).await
    }

    /// `POST` `body` to `path` (relative to the base) and decode the response.
    pub async fn post_json<B: Serialize, T: DeserializeOwned>(
        &self,
        mb: &MailboxCtx,
        path: &str,
        query: &[(&str, String)],
        body: &B,
    ) -> Result<T, MailError> {
        let raw = serde_json::to_vec(body).map_err(|_| MailError::Transient)?;
        self.send(mb, HttpMethod::Post, path, query, Some(raw))
            .await
    }

    /// The clock, for HTTP-date `Retry-After` values.
    pub fn now(&self) -> time::OffsetDateTime {
        self.clock.now()
    }

    #[allow(clippy::too_many_arguments)]
    async fn send<T: DeserializeOwned>(
        &self,
        mb: &MailboxCtx,
        method: HttpMethod,
        path: &str,
        query: &[(&str, String)],
        body: Option<Vec<u8>>,
    ) -> Result<T, MailError> {
        let url = self.build_url(path, query);
        let mut headers = vec![
            (
                "Authorization".to_owned(),
                Sensitive::new(format!("Bearer {}", mb.access_token.expose())),
            ),
            (
                "Accept".to_owned(),
                Sensitive::new("application/json".to_owned()),
            ),
        ];
        if body.is_some() {
            headers.push((
                "Content-Type".to_owned(),
                Sensitive::new("application/json".to_owned()),
            ));
        }
        let request = EgressRequest {
            method,
            url,
            headers,
            body,
            timeout: Duration::from_secs(GMAIL_TIMEOUT_S),
        };
        let response = match self.egress.call(request).await {
            Ok(r) => r,
            Err(e) => {
                log_op(route_for(path), "failure", None);
                return Err(map_egress_error(&e));
            }
        };
        if response.body.len() > MAX_RESPONSE_BYTES {
            log_op(route_for(path), "failure", Some(response.status));
            return Err(MailError::Transient);
        }
        let status = response.status;
        if (200..300).contains(&status) {
            match serde_json::from_slice::<T>(&response.body) {
                Ok(decoded) => {
                    log_op(route_for(path), "success", Some(status));
                    Ok(decoded)
                }
                Err(_) => {
                    log_op(route_for(path), "failure", Some(status));
                    Err(MailError::Transient)
                }
            }
        } else {
            let err = map_gmail_error(
                status,
                retry_after(&response.headers),
                &response.body,
                self.clock.now(),
            );
            log_op(route_for(path), "failure", Some(status));
            Err(err)
        }
    }

    /// The absolute URL for `path` plus the query pairs, never string-built.
    fn build_url(&self, path: &str, query: &[(&str, String)]) -> Url {
        let base_path = self.base.path().trim_end_matches('/');
        let full = format!("{base_path}/{}", path.trim_start_matches('/'));
        let mut url = self.base.clone();
        url.set_path(&full);
        url.set_query(None);
        {
            let mut pairs = url.query_pairs_mut();
            for (key, value) in query {
                pairs.append_pair(key, value);
            }
        }
        url
    }
}

/// Log one Gmail call: route and status only, never the ID, query or a header.
fn log_op(route: &'static str, outcome: &'static str, status: Option<u16>) {
    op_log(&OpLog {
        op: route,
        outcome,
        status,
        latency_ms: None,
    });
}

/// The registered operation name for a request path.
fn route_for(path: &str) -> &'static str {
    if path == "messages" {
        "gmail.messages.list"
    } else if path.starts_with("labels/") {
        "gmail.labels.list"
    } else {
        "gmail.messages.get"
    }
}

/// The `Retry-After` header value, case-insensitive.
fn retry_after(headers: &[(String, String)]) -> Option<&str> {
    headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("retry-after"))
        .map(|(_, v)| v.as_str())
}

/// An egress refusal is a configuration bug (`Invalid`), never retried; every
/// other transport failure is transient.
fn map_egress_error(err: &EgressError) -> MailError {
    match err {
        EgressError::HostNotAllowed | EgressError::NotPermitted => {
            MailError::Invalid("egress_refused".to_owned())
        }
        // nosemgrep: mailtinder-no-permanent-delete -- maps a refused delete to a config error; never issues one.
        EgressError::PermanentDeleteRefused => MailError::Invalid("egress_refused".to_owned()),
        _ => MailError::Transient,
    }
}
