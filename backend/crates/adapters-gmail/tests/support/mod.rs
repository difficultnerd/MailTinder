//! Shared test support for the `adapters-gmail` integration tests.
//!
//! Production calls go through the T-306 egress allowlist; these tests use a
//! local pass-through egress that forwards the full URL (path *and* query) to
//! `fake-google` over plain http, as the task's edge cases permit.
#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::missing_panics_doc,
    clippy::missing_errors_doc
)]

use std::sync::Arc;

use adapters_gmail::{DriveAppFolder, GmailHttp, GmailProvider};
use async_trait::async_trait;
use domain::MailboxId;
use fake_google::{FakeGoogle, FakeGoogleHandle, FakeMailboxKey, GMAIL_MODIFY};
use obs::Sensitive;
use ports::{
    Clock, EgressError, EgressRequest, EgressResponse, HttpEgress, HttpMethod, MailboxCtx,
    OneClickOutcome,
};
use testkit::clock::VirtualClock;
use testkit::contract::mail_provider::MailTarget;
use url::Url;

/// A pass-through egress: sends the request to the URL as given.
pub struct PassthroughEgress;

#[async_trait]
impl HttpEgress for PassthroughEgress {
    async fn one_click_post(&self, _url: &Url) -> Result<OneClickOutcome, EgressError> {
        Err(EgressError::NotPermitted)
    }

    async fn call(&self, req: EgressRequest) -> Result<EgressResponse, EgressError> {
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .timeout(req.timeout)
            .build()
            .map_err(|_| EgressError::Connect)?;
        let mut builder = match req.method {
            HttpMethod::Get => client.get(req.url.clone()),
            HttpMethod::Post => client.post(req.url.clone()),
            HttpMethod::Put => client.put(req.url.clone()),
            HttpMethod::Patch => client.patch(req.url.clone()),
            HttpMethod::Delete => client.delete(req.url.clone()),
        };
        for (key, value) in &req.headers {
            builder = builder.header(key, value.expose());
        }
        if let Some(body) = &req.body {
            builder = builder.body(body.clone());
        }
        let resp = builder.send().await.map_err(|_| EgressError::Connect)?;
        let status = resp.status().as_u16();
        let headers = resp
            .headers()
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or_default().to_owned()))
            .collect();
        let body = resp
            .bytes()
            .await
            .map_err(|_| EgressError::Connect)?
            .to_vec();
        Ok(EgressResponse {
            status,
            headers,
            body,
        })
    }
}

/// Start a `fake-google` on a virtual clock fixed at `testkit::T0`.
pub async fn start() -> FakeGoogleHandle {
    let clock: Arc<dyn Clock> = Arc::new(VirtualClock::new(testkit::T0));
    FakeGoogle::start(clock).await.expect("start fake-google")
}

/// Start a `fake-google` on a caller-held virtual clock, so a test can advance
/// time (for example to make two app-folder files differ by `modifiedTime`).
pub async fn start_on_clock(clock: Arc<VirtualClock>) -> FakeGoogleHandle {
    FakeGoogle::start(clock).await.expect("start fake-google")
}

/// A Drive app-folder store pointed at the fake's root.
pub fn drive_store(handle: &FakeGoogleHandle, clock: Arc<dyn Clock>) -> DriveAppFolder {
    DriveAppFolder::new(GmailHttp::new(
        Arc::new(PassthroughEgress),
        handle.base_url(),
        clock,
    ))
}

/// A provider pointed at the fake, sharing the given clock.
pub fn provider(handle: &FakeGoogleHandle, clock: Arc<dyn Clock>) -> GmailProvider {
    let base = handle
        .base_url()
        .join("gmail/v1/users/me")
        .expect("base url");
    GmailProvider::new(GmailHttp::new(Arc::new(PassthroughEgress), base, clock))
}

/// A mailbox context with a freshly issued `gmail.modify` token.
pub fn mail_ctx(handle: &FakeGoogleHandle, mb: &FakeMailboxKey) -> MailboxCtx {
    let token = handle.issue_token(mb, &[GMAIL_MODIFY], time::Duration::hours(1));
    MailboxCtx {
        mailbox: MailboxId(uuid::Uuid::from_u128(1)),
        access_token: Sensitive::new(token),
    }
}

/// A clock fixed at the testkit epoch.
pub fn clock() -> Arc<dyn Clock> {
    Arc::new(VirtualClock::new(testkit::T0))
}

/// A contract-suite target on a fresh mailbox with the given email.
pub fn target(handle: &FakeGoogleHandle, email: &str) -> MailTarget {
    let mb = handle.add_mailbox(email);
    MailTarget {
        provider: Arc::new(provider(handle, clock())),
        ctx: mail_ctx(handle, &mb),
        seeder: handle.seeder(&mb),
    }
}

/// A refusing egress: every call is refused as a disallowed host.
pub struct RefusingEgress;

#[async_trait]
impl HttpEgress for RefusingEgress {
    async fn one_click_post(&self, _url: &Url) -> Result<OneClickOutcome, EgressError> {
        Err(EgressError::HostNotAllowed)
    }

    async fn call(&self, _req: EgressRequest) -> Result<EgressResponse, EgressError> {
        Err(EgressError::HostNotAllowed)
    }
}

/// A provider whose egress refuses every call.
pub fn refusing_provider(handle: &FakeGoogleHandle) -> GmailProvider {
    let base = handle
        .base_url()
        .join("gmail/v1/users/me")
        .expect("base url");
    GmailProvider::new(GmailHttp::new(Arc::new(RefusingEgress), base, clock()))
}
