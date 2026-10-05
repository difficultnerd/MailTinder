//! API rate policies and counters, using only hashed store keys.
use crate::error::ApiError;
use crate::http::{client_ip::ClientIp, request_id::RequestId};
use domain::{MailboxId, UserId};
use hmac::{Hmac, Mac};
use obs::Sensitive;
use ports::{Clock, ServerStore, SessionRecordId, StoreError};
use sha2::Sha256;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use time::{Duration, OffsetDateTime};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyKind {
    Ip,
    User,
    Session,
    Mailbox,
    EmailHash,
    Subject,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StoreKind {
    Memory,
    Firestore,
}
#[derive(Clone, Copy, Debug)]
pub struct Policy {
    pub name: &'static str,
    pub limit: u32,
    pub window_s: u64,
    pub burst: Option<u32>,
    pub key: KeyKind,
    pub store: StoreKind,
}

pub mod policies {
    use super::{KeyKind, Policy, StoreKind};
    macro_rules! policy {
        ($id:ident,$name:literal,$limit:literal,$seconds:literal,$burst:expr,$key:ident,$store:ident) => {
            pub const $id: Policy = Policy {
                name: $name,
                limit: $limit,
                window_s: $seconds,
                burst: $burst,
                key: KeyKind::$key,
                store: StoreKind::$store,
            };
        };
    }
    policy!(SIGN_IN_IP, "sign_in_ip", 20, 600, None, Ip, Firestore);
    policy!(
        SIGN_IN_FAILED_SUBJECT,
        "sign_in_failed",
        10,
        3600,
        None,
        Subject,
        Firestore
    );
    policy!(
        INVITE_REQUEST_IP,
        "invite_request_ip",
        5,
        3600,
        None,
        Ip,
        Firestore
    );
    policy!(
        INVITE_REQUEST_EMAIL,
        "invite_request_email",
        1,
        86400,
        None,
        EmailHash,
        Firestore
    );
    policy!(SWIPES, "swipes", 60, 60, Some(10), User, Memory);
    policy!(FEED, "feed", 30, 60, None, User, Memory);
    policy!(UNSUB_JOBS, "unsub_jobs", 300, 86400, None, User, Firestore);
    policy!(MAILTO, "mailto", 100, 86400, None, Mailbox, Firestore);
    policy!(OTHER_READS, "reads", 120, 60, None, Session, Memory);
    policy!(OTHER_WRITES, "writes", 30, 60, None, User, Memory);
    policy!(
        ADMIN_INVITE_SENDS,
        "admin_invites",
        50,
        86400,
        None,
        User,
        Firestore
    );
    policy!(
        ADMIN_SESSION_KILL,
        "admin_session_kill",
        20,
        86400,
        None,
        User,
        Firestore
    );
    policy!(
        ACCOUNT_DELETE,
        "account_delete",
        3,
        86400,
        None,
        User,
        Firestore
    );
    policy!(
        EXPERIMENT_OPT,
        "experiment_opt",
        10,
        86400,
        None,
        User,
        Firestore
    );
    policy!(
        EXPERIMENT_ADMIN,
        "experiment_admin",
        50,
        86400,
        None,
        User,
        Firestore
    );
    policy!(
        BAKEOFF_REPORT,
        "bakeoff_report",
        30,
        3600,
        None,
        User,
        Memory
    );
    policy!(
        SNAPSHOT_SAVE,
        "snapshot_save",
        20,
        86400,
        None,
        User,
        Firestore
    );
    policy!(
        SNAPSHOT_DELETE,
        "snapshot_delete",
        50,
        86400,
        None,
        User,
        Firestore
    );
}

#[derive(Clone, Copy)]
pub enum LimitSubject<'a> {
    Ip(&'a ClientIp),
    User(&'a UserId),
    Session(&'a SessionRecordId),
    Mailbox(&'a MailboxId),
    Email(&'a str),
    Subject(&'a str),
}
#[derive(Clone, Copy, Debug)]
pub struct LimitInfo {
    pub policy: &'static Policy,
    pub remaining: u32,
    pub reset_s: u64,
}

struct Window {
    start: i64,
    count: u32,
    tokens: u128,
    last: i64,
    period_s: u64,
}
pub struct RateLimiter {
    store: Arc<dyn ServerStore>,
    clock: Arc<dyn Clock>,
    key: Sensitive<Vec<u8>>,
    memory: Mutex<HashMap<String, Window>>,
}
impl RateLimiter {
    #[must_use]
    pub fn new(
        store: Arc<dyn ServerStore>,
        clock: Arc<dyn Clock>,
        key: Sensitive<Vec<u8>>,
    ) -> Self {
        Self {
            store,
            clock,
            key,
            memory: Mutex::new(HashMap::new()),
        }
    }
    /// Counts a hit and returns rate metadata, or a limit/internal error.
    /// # Errors
    /// A limit is exceeded, or a security-critical store operation fails.
    pub async fn check(
        &self,
        policy: &'static Policy,
        subject: LimitSubject<'_>,
        request_id: RequestId,
    ) -> Result<LimitInfo, ApiError> {
        let now = self.clock.now().unix_timestamp();
        let seconds = i64::try_from(policy.window_s).map_err(|_| ApiError::Internal)?;
        if seconds == 0 {
            return Err(ApiError::Internal);
        }
        let start = now.div_euclid(seconds) * seconds;
        let key = self.rate_key(policy, subject)?;
        let count = match policy.store {
            StoreKind::Memory => self.memory_hit(policy, &key, start, now)?,
            StoreKind::Firestore => {
                let stamp =
                    OffsetDateTime::from_unix_timestamp(start).map_err(|_| ApiError::Internal)?;
                match self
                    .store
                    .rate_limits()
                    .hit(&ports::RateLimitKey(key), stamp, Duration::seconds(seconds))
                    .await
                {
                    Ok(n) => n,
                    Err(StoreError::Unavailable) if read_policy(policy) => 1,
                    Err(_) => return Err(ApiError::Internal),
                }
            }
        };
        let remaining = policy.burst.unwrap_or(policy.limit).saturating_sub(count);
        if count > policy.burst.unwrap_or(policy.limit) {
            obs::security_event(&obs::SecurityEvent {
                action: "rate_limit_hit",
                outcome: "refused",
                user: None,
                request_id: Some(request_id.0),
                amr: None,
                provider: None,
                method: None,
            });
            return Err(ApiError::RateLimited {
                retry_after_s: u64::try_from((start + seconds - now).max(1)).unwrap_or(1),
            });
        }
        let reset_s = u64::try_from(start + seconds - now).unwrap_or(1);
        Ok(LimitInfo {
            policy,
            remaining,
            reset_s,
        })
    }
    fn memory_hit(
        &self,
        policy: &Policy,
        key: &str,
        start: i64,
        now: i64,
    ) -> Result<u32, ApiError> {
        let mut map = self.memory.lock().map_err(|_| ApiError::Internal)?;
        if map.len() > 10_000 {
            map.retain(|_, w| {
                w.start
                    .saturating_add(i64::try_from(w.period_s).unwrap_or(i64::MAX))
                    > now
            });
        }
        let cap = policy.burst.unwrap_or(policy.limit);
        let w = map.entry(key.to_owned()).or_insert(Window {
            start,
            count: 0,
            tokens: u128::from(cap) * u128::from(policy.window_s),
            last: now,
            period_s: policy.window_s,
        });
        if policy.burst.is_some() {
            let elapsed = u128::try_from(now.saturating_sub(w.last)).unwrap_or(0);
            let maximum = u128::from(cap) * u128::from(policy.window_s);
            w.tokens = w
                .tokens
                .saturating_add(elapsed.saturating_mul(u128::from(policy.limit)))
                .min(maximum);
            w.last = now;
            if w.tokens < u128::from(policy.window_s) {
                return Ok(cap.saturating_add(1));
            }
            w.tokens -= u128::from(policy.window_s);
            return Ok(cap.saturating_sub(
                u32::try_from(w.tokens / u128::from(policy.window_s)).unwrap_or(cap),
            ));
        }
        if w.start != start {
            w.start = start;
            w.count = 0;
        }
        w.count = w.count.saturating_add(1);
        Ok(w.count)
    }
    fn rate_key(&self, policy: &Policy, subject: LimitSubject<'_>) -> Result<String, ApiError> {
        let subject = match subject {
            LimitSubject::Ip(ip) => ip.0.map_or_else(|| "unknown".to_owned(), |v| v.to_string()),
            LimitSubject::User(v) => v.0.to_string(),
            LimitSubject::Session(v) => v.0.to_string(),
            LimitSubject::Mailbox(v) => v.0.to_string(),
            LimitSubject::Email(v) => v.to_lowercase(),
            LimitSubject::Subject(v) => v.to_owned(),
        };
        let mut mac =
            Hmac::<Sha256>::new_from_slice(self.key.expose()).map_err(|_| ApiError::Internal)?;
        mac.update(subject.as_bytes());
        let digest = hex::encode(mac.finalize().into_bytes());
        Ok(format!("{}:{}", policy.name, &digest[..32]))
    }
}
fn read_policy(policy: &Policy) -> bool {
    matches!(policy.name, "reads" | "feed" | "bakeoff_report")
}
#[must_use]
pub fn default_policy(method: &axum::http::Method) -> Option<&'static Policy> {
    match *method {
        axum::http::Method::GET => Some(&policies::OTHER_READS),
        axum::http::Method::POST
        | axum::http::Method::PUT
        | axum::http::Method::PATCH
        | axum::http::Method::DELETE => Some(&policies::OTHER_WRITES),
        _ => None,
    }
}
pub fn apply_limit_headers(resp: &mut axum::response::Response, info: &LimitInfo) {
    let policy = format!(
        "\"{}\";q={};w={}",
        info.policy.name, info.policy.limit, info.policy.window_s
    );
    let limit = format!(
        "\"{}\";r={};t={}",
        info.policy.name, info.remaining, info.reset_s
    );
    if let (Ok(p), Ok(l)) = (policy.parse(), limit.parse()) {
        resp.headers_mut().insert("ratelimit-policy", p);
        resp.headers_mut().insert("ratelimit", l);
    }
}
