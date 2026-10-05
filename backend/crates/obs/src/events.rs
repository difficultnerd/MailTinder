//! Typed helpers that emit log events with the exact allowlisted key names.

use crate::PseudoId;

/// A request log line (event `request`).
pub struct RequestLog {
    pub request_id: uuid::Uuid,
    pub user: Option<PseudoId>,
    pub route: &'static str,
    pub status: u16,
    pub latency_ms: u64,
    pub rate_limit_hit: bool,
}

/// A security event (event `security`, severity NOTICE).
pub struct SecurityEvent {
    pub action: &'static str,
    pub outcome: &'static str,
    pub user: Option<PseudoId>,
    pub request_id: Option<uuid::Uuid>,
    pub amr: Option<Vec<String>>,
    pub provider: Option<&'static str>,
    pub method: Option<&'static str>,
}

/// A metric event (event `metric`).
pub struct MetricEvent {
    pub event_type: &'static str,
    pub outcome: &'static str,
    pub user: Option<PseudoId>,
    pub provider: Option<&'static str>,
}

/// A non-HTTP operation log (event `op`).
pub struct OpLog {
    pub op: &'static str,
    pub outcome: &'static str,
    pub status: Option<u16>,
    pub latency_ms: Option<u64>,
}

pub fn request_log(e: &RequestLog) {
    let user = e.user.as_ref().map_or("", PseudoId::as_str);
    tracing::event!(
        tracing::Level::INFO,
        event = "request",
        request_id = e.request_id.to_string(),
        user_pseudo = user,
        route = e.route,
        status = e.status,
        latency_ms = e.latency_ms,
        rate_limit_hit = e.rate_limit_hit,
    );
}

pub fn security_event(e: &SecurityEvent) {
    let user = e.user.as_ref().map_or("", PseudoId::as_str);
    let request_id = e.request_id.map(|u| u.to_string()).unwrap_or_default();
    let amr = e.amr.as_deref().map(|v| v.join(",")).unwrap_or_default();
    tracing::event!(
        tracing::Level::WARN,
        event = "security",
        action = e.action,
        outcome = e.outcome,
        user_pseudo = user,
        request_id = request_id,
        amr = amr,
        provider = e.provider.unwrap_or(""),
        method = e.method.unwrap_or(""),
    );
}

pub fn metric_event(e: &MetricEvent) {
    let user = e.user.as_ref().map_or("", PseudoId::as_str);
    tracing::event!(
        tracing::Level::INFO,
        event = "metric",
        action = e.event_type,
        outcome = e.outcome,
        user_pseudo = user,
        provider = e.provider.unwrap_or(""),
    );
}

pub fn op_log(e: &OpLog) {
    match e.status {
        Some(s) if (200..300).contains(&s) => {
            tracing::event!(
                tracing::Level::DEBUG,
                event = "op",
                route = e.op,
                outcome = e.outcome,
                status = e.status.unwrap_or(0),
                latency_ms = e.latency_ms.unwrap_or(0),
            );
        }
        _ => {
            tracing::event!(
                tracing::Level::WARN,
                event = "op",
                route = e.op,
                outcome = e.outcome,
                status = e.status.unwrap_or(0),
                latency_ms = e.latency_ms.unwrap_or(0),
            );
        }
    }
}
