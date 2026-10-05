//! Global initialisation: the redacting subscriber and the panic hook.

use std::sync::Arc;

use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

use crate::layer::AllowlistJsonLayer;
use crate::registry;
use crate::sink::{CaptureSink, LogSink};

/// An error from `init`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObsError;

impl std::fmt::Display for ObsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("obs already initialised")
    }
}

impl std::error::Error for ObsError {}

static INIT: std::sync::OnceLock<Result<(), ObsError>> = std::sync::OnceLock::new();

/// Initialise the global redacting subscriber and panic hook. Call once per
/// process. In release builds the panic hook writes no payload.
///
/// # Errors
///
/// Returns `ObsError` if `init` has already been called in this process.
pub fn init(service: &'static str, sink: Arc<dyn LogSink>) -> Result<(), ObsError> {
    INIT.get_or_init(|| {
        let subscriber =
            tracing_subscriber::registry().with(AllowlistJsonLayer::new(service, sink.clone()));
        subscriber.init();
        install_panic_hook(service, sink);
        Ok(())
    })
    .clone()
}

fn install_panic_hook(service: &'static str, sink: Arc<dyn LogSink>) {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        // One line, no payload, no location. Release builds never print the
        // panic message, which can hold data.
        let line = format!(
            "{{\"time\":\"{}\",\"severity\":\"CRITICAL\",\"service\":\"{}\",\"event\":\"panic\"}}",
            now_rfc3339(),
            service
        );
        sink.write_line(&line);
        #[cfg(debug_assertions)]
        prev(info);
    }));
}

/// For tests: a scoped subscriber on the current thread. Returns the capture
/// sink and a guard that restores the previous subscriber when dropped.
#[must_use]
pub fn capture(service: &'static str) -> (CaptureSink, tracing::subscriber::DefaultGuard) {
    let sink = CaptureSink::default();
    let subscriber = tracing_subscriber::registry()
        .with(AllowlistJsonLayer::new(service, Arc::new(sink.clone())));
    let guard = tracing::subscriber::set_default(subscriber);
    (sink, guard)
}

/// Number of values replaced because they were not in a registry.
#[must_use]
pub fn unregistered_count() -> u64 {
    registry::unregistered_count()
}

fn now_rfc3339() -> String {
    let now = time::OffsetDateTime::now_utc();
    now.format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_owned())
}
