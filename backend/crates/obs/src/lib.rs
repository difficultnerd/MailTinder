//! Structured logging, redaction types, pseudonymous IDs.
//!
//! `obs` is the only way anything reaches a log line. A redacting `tracing`
//! layer writes one JSON object per event to a sink, keeping only the
//! allowlisted fields with validated shapes; every other field and any
//! free-text message is dropped. User IDs appear only as an HMAC pseudonym.

pub mod clock;
pub mod events;
pub mod init;
pub mod layer;
pub mod pseudo;
pub mod registry;
pub mod scan;
pub mod sensitive;
pub mod sink;

pub use clock::{arc, Clock, FixedClock};
pub use events::{
    metric_event, op_log, request_log, security_event, MetricEvent, OpLog, RequestLog,
    SecurityEvent,
};
pub use init::{capture, init, unregistered_count};
pub use layer::AllowlistJsonLayer;
pub use pseudo::{PseudoId, Pseudonymiser};
pub use registry::register_http_routes;
pub use scan::scan_for_leaks;
pub use sensitive::Sensitive;
pub use sink::{CaptureSink, LogSink, StdoutSink};
