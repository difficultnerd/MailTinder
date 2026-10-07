//! The redacting `tracing` layer: writes one JSON object per event to a sink,
//! keeping only allowlisted fields with validated shapes.

use std::sync::Arc;

use serde_json::{json, Map, Value};
use tracing::field::{Field, Visit};
use tracing::{Event, Subscriber};
use tracing_subscriber::layer::Context;
use tracing_subscriber::Layer;

use crate::clock::Clock;
use crate::registry;
use crate::sink::LogSink;

/// The set of keys a log line may carry, and how each is validated.
/// `None` means "must be in a registry"; `Some(shape)` is a shape check.
enum Check {
    /// Value must be in the named registry list.
    Registry(&'static [&'static str]),
    /// Value must parse as a UUID.
    Uuid,
    /// Value must be exactly 32 lower-case hex chars.
    Pseudo,
    /// Value must be an integer in the given range.
    Int(i64, i64),
    /// Any bool.
    Bool,
    /// Any unsigned integer.
    UInt,
    /// Comma-joined AMR values; each must be in `AMR_VALUES`, others become "other".
    Amr,
}

fn checks() -> &'static [(&'static str, Check)] {
    &[
        ("event", Check::Registry(registry::EVENTS)),
        ("request_id", Check::Uuid),
        ("user_pseudo", Check::Pseudo),
        ("target_user_pseudo", Check::Pseudo),
        ("route", Check::Registry(registry::OPS)),
        ("status", Check::Int(100, 599)),
        ("latency_ms", Check::UInt),
        ("action", Check::Registry(registry::ACTIONS)),
        ("outcome", Check::Registry(registry::OUTCOMES)),
        ("rate_limit_hit", Check::Bool),
        ("amr", Check::Amr),
        ("provider", Check::Registry(registry::PROVIDERS)),
        ("method", Check::Registry(registry::JOB_METHODS)),
    ]
}

/// The redacting layer. `service` is the fixed service name for every line.
pub struct AllowlistJsonLayer {
    service: &'static str,
    sink: Arc<dyn LogSink>,
    clock: Arc<dyn Clock>,
}

impl AllowlistJsonLayer {
    pub fn new(service: &'static str, sink: Arc<dyn LogSink>, clock: Arc<dyn Clock>) -> Self {
        Self {
            service,
            sink,
            clock,
        }
    }
}

impl<S: Subscriber> Layer<S> for AllowlistJsonLayer {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let mut visitor = AllowlistVisitor::default();
        event.record(&mut visitor);
        let line = render_line(
            self.service,
            *event.metadata().level(),
            &visitor,
            &*self.clock,
        );
        self.sink.write_line(&line);
    }
}

/// Collects the allowlisted fields from an event, dropping everything else.
#[derive(Default)]
struct AllowlistVisitor {
    fields: Map<String, Value>,
    dropped: u64,
}

impl Visit for AllowlistVisitor {
    fn record_str(&mut self, field: &Field, value: &str) {
        let v = Value::String(value.to_owned());
        self.record_value(field, &v);
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        let v = Value::from(value);
        self.record_value(field, &v);
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        let v = Value::from(value);
        self.record_value(field, &v);
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        let v = Value::Bool(value);
        self.record_value(field, &v);
    }

    fn record_debug(&mut self, _field: &Field, _value: &dyn std::fmt::Debug) {
        // Never record Debug-formatted values; they can hold anything.
    }
}

impl AllowlistVisitor {
    fn record_value(&mut self, field: &Field, value: &Value) {
        let name = field.name();
        // The `message` field and any unknown key are dropped.
        let Some((_, check)) = checks().iter().find(|(k, _)| *k == name) else {
            self.dropped += 1;
            return;
        };
        let validated = validate(check, value);
        if validated.is_none() {
            self.dropped += 1;
        }
        if let Some(v) = validated {
            self.fields.insert(name.to_owned(), v);
        }
    }
}

/// Validate a value against its check. Returns `None` if it must be dropped.
fn validate(check: &Check, value: &Value) -> Option<Value> {
    match check {
        Check::Registry(list) => match value {
            Value::String(s) if registry::in_list(list, s) => Some(value.clone()),
            _ => {
                registry::count_unregistered();
                None
            }
        },
        Check::Uuid => match value {
            Value::String(s) if uuid::Uuid::parse_str(s).is_ok() => Some(value.clone()),
            _ => None,
        },
        Check::Pseudo => match value {
            Value::String(s)
                if s.len() == 32
                    && s.chars()
                        .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()) =>
            {
                Some(value.clone())
            }
            _ => None,
        },
        Check::Int(lo, hi) => match value {
            Value::Number(n) => n
                .as_i64()
                .filter(|v| *v >= *lo && *v <= *hi)
                .map(|_| value.clone()),
            _ => None,
        },
        Check::UInt => match value {
            Value::Number(n) if n.as_u64().is_some() => Some(value.clone()),
            _ => None,
        },
        Check::Bool => match value {
            Value::Bool(_) => Some(value.clone()),
            _ => None,
        },
        Check::Amr => match value {
            Value::String(s) => {
                let parts: Vec<&str> = s.split(',').collect();
                let mapped: Vec<&str> = parts
                    .iter()
                    .map(|p| {
                        if registry::in_list(registry::AMR_VALUES, p) {
                            p
                        } else {
                            "other"
                        }
                    })
                    .collect();
                Some(Value::String(mapped.join(",")))
            }
            _ => None,
        },
    }
}

/// Build the one-line JSON object for an event.
fn render_line(
    service: &'static str,
    level: tracing::Level,
    v: &AllowlistVisitor,
    clock: &dyn Clock,
) -> String {
    let event = v
        .fields
        .get("event")
        .and_then(Value::as_str)
        .unwrap_or("op");
    // The security_event helper is NOTICE; everything else maps from the level.
    let severity = if event == "security" {
        "NOTICE"
    } else {
        match level {
            tracing::Level::TRACE | tracing::Level::DEBUG => "DEBUG",
            tracing::Level::INFO => "INFO",
            tracing::Level::WARN => "WARNING",
            tracing::Level::ERROR => "ERROR",
        }
    };
    let mut obj = Map::new();
    obj.insert("time".to_owned(), json!(now_rfc3339(clock)));
    obj.insert("severity".to_owned(), json!(severity));
    obj.insert("service".to_owned(), json!(service));
    obj.insert("event".to_owned(), json!(event));
    for (k, val) in &v.fields {
        if k != "event" {
            obj.insert(k.clone(), val.clone());
        }
    }
    if v.dropped > 0 {
        obj.insert("dropped_fields".to_owned(), json!(v.dropped));
    }
    serde_json::to_string(&Value::Object(obj)).unwrap_or_else(|_| "{}".to_owned())
}

fn now_rfc3339(clock: &dyn Clock) -> String {
    let now = clock.now();
    now.format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_owned())
}
