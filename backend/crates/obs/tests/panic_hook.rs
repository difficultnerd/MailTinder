#![allow(clippy::expect_used)]

//! Panic hook test: a panic whose message holds a canary produces one `panic`
//! line without it. Separate binary because it calls the global `init`.

use obs::{init, CaptureSink};
use std::sync::Arc;

#[test]
fn panic_hook_writes_no_payload() {
    let sink = CaptureSink::default();
    init("test", Arc::new(sink.clone())).expect("init once");
    let result = std::panic::catch_unwind(|| {
        panic!("CANARY-SECRET-123");
    });
    assert!(result.is_err());
    let lines = sink.lines();
    assert_eq!(lines.len(), 1, "exactly one panic line");
    let line = &lines[0];
    assert!(line.contains("\"event\":\"panic\""));
    assert!(line.contains("\"severity\":\"CRITICAL\""));
    assert!(!line.contains("CANARY-SECRET-123"), "panic payload leaked");
    // No location either.
    assert!(!line.contains("panic_hook.rs"));
}
