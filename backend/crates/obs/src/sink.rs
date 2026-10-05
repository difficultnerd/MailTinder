//! Where log lines go. A sink writes exactly one line at a time.

use std::io::Write;
use std::sync::{Arc, Mutex};

/// A destination for log lines. Implementations must write exactly one line.
pub trait LogSink: Send + Sync + 'static {
    fn write_line(&self, line: &str);
}

/// Writes to locked stdout, one line, flushed.
pub struct StdoutSink;

impl LogSink for StdoutSink {
    fn write_line(&self, line: &str) {
        let mut out = std::io::stdout().lock();
        let _ = writeln!(out, "{line}");
        let _ = out.flush();
    }
}

/// Collects lines in memory for tests.
#[derive(Clone, Default)]
pub struct CaptureSink {
    lines: Arc<Mutex<Vec<String>>>,
}

impl CaptureSink {
    #[must_use]
    pub fn lines(&self) -> Vec<String> {
        self.lines
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    #[must_use]
    pub fn text(&self) -> String {
        self.lines().join("\n")
    }
}

impl LogSink for CaptureSink {
    fn write_line(&self, line: &str) {
        self.lines
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(line.to_owned());
    }
}

/// Convenience: an `Arc<dyn LogSink>` from a concrete sink.
pub fn arc<S: LogSink>(sink: S) -> Arc<dyn LogSink> {
    Arc::new(sink)
}
