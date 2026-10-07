#![allow(clippy::expect_used)]

//! T-307 redaction tests: prove no canary, address or URL reaches log output.

use obs::{
    capture, metric_event, op_log, request_log, scan_for_leaks, security_event, MetricEvent, OpLog,
    PseudoId, Pseudonymiser, RequestLog, Sensitive,
};

fn pseudo() -> PseudoId {
    Pseudonymiser::new(Sensitive::new(vec![7u8; 32])).pseudo_id(&uuid::Uuid::new_v4())
}

fn parse_lines(text: &str) -> Vec<serde_json::Value> {
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("each line is one JSON object"))
        .collect()
}

fn test_clock() -> std::sync::Arc<dyn obs::Clock> {
    obs::arc(obs::FixedClock::default())
}

#[test]
fn xc_01_unknown_fields_and_messages_are_dropped() {
    let (sink, _guard) = capture("test", test_clock());
    // nosemgrep: privacy-log-sensitive-identifier -- test fixture: prove redaction drops these
    tracing::info!(
        email = "a@example.com",
        url = "https://x.example.com",
        "hello {}",
        "CANARY-x"
    );
    let text = sink.text();
    assert!(text.contains("dropped_fields"));
    assert!(!text.contains("a@example.com"));
    assert!(!text.contains("x.example.com"));
    assert!(!text.contains("CANARY-x"));
    assert!(!text.contains("hello"));
}

#[test]
fn xc_01_helpers_emit_only_allowlisted_keys() {
    let (sink, _guard) = capture("test", test_clock());
    request_log(&RequestLog {
        request_id: uuid::Uuid::new_v4(),
        user: Some(pseudo()),
        route: "/api/v1/swipes",
        status: 200,
        latency_ms: 12,
        rate_limit_hit: false,
    });
    security_event(&obs::SecurityEvent {
        action: "sign_in",
        outcome: "success",
        user: Some(pseudo()),
        request_id: Some(uuid::Uuid::new_v4()),
        amr: Some(vec!["pwd".to_owned()]),
        provider: Some("gmail"),
        method: None,
    });
    metric_event(&MetricEvent {
        event_type: "swipe",
        outcome: "success",
        user: Some(pseudo()),
        provider: Some("gmail"),
    });
    op_log(&OpLog {
        op: "gmail.messages.list",
        outcome: "success",
        status: Some(200),
        latency_ms: Some(5),
    });
    let allowed = [
        "time",
        "severity",
        "service",
        "event",
        "request_id",
        "user_pseudo",
        "target_user_pseudo",
        "route",
        "status",
        "latency_ms",
        "action",
        "outcome",
        "rate_limit_hit",
        "amr",
        "provider",
        "method",
        "dropped_fields",
    ];
    for line in parse_lines(&sink.text()) {
        let obj = line.as_object().expect("object");
        for key in obj.keys() {
            assert!(allowed.contains(&key.as_str()), "unexpected key {key}");
        }
    }
}

#[test]
fn log_1_canaries_never_reach_log_output() {
    let (sink, _guard) = capture("test", test_clock());
    let canaries = vec![
        "CANARY-1".to_owned(),
        "a@example.com".to_owned(),
        "https://leak.example.com".to_owned(),
    ];
    // Free text.
    tracing::info!("free text {}", "CANARY-1");
    // Unknown keys.
    // nosemgrep: privacy-log-sensitive-identifier -- test fixture: prove redaction drops these
    tracing::info!(subject = "a@example.com", body = "CANARY-1");
    // Allowed keys with bad values.
    tracing::info!(
        action = "CANARY-1",
        route = "/api/v1/messages/CANARY-1",
        user_pseudo = "a@example.com"
    );
    // Debug of Sensitive.
    let s = Sensitive::new("a@example.com");
    // nosemgrep: privacy-log-sensitive-identifier -- test fixture: prove redaction drops these
    tracing::info!(secret = ?s);
    let leaks = scan_for_leaks(&sink.text(), &canaries);
    assert!(leaks.is_empty(), "leaks found: {leaks:?}");
}

#[test]
fn asvs_v16_2_1_security_event_has_required_metadata() {
    let (sink, _guard) = capture("test", test_clock());
    let p = pseudo();
    security_event(&obs::SecurityEvent {
        action: "sign_in",
        outcome: "success",
        user: Some(p),
        request_id: Some(uuid::Uuid::new_v4()),
        amr: Some(vec!["pwd".to_owned()]),
        provider: Some("gmail"),
        method: None,
    });
    let line = parse_lines(&sink.text()).pop().expect("one line");
    let obj = line.as_object().expect("object");
    assert_eq!(obj["severity"], "NOTICE");
    assert_eq!(obj["event"], "security");
    assert_eq!(obj["action"], "sign_in");
    assert_eq!(obj["outcome"], "success");
    assert!(obj.contains_key("user_pseudo"));
    assert!(obj.contains_key("request_id"));
    assert!(obj.contains_key("time"));
    assert!(obj.contains_key("service"));
}

#[test]
fn asvs_v16_2_4_each_line_is_one_json_object() {
    let (sink, _guard) = capture("test", test_clock());
    request_log(&RequestLog {
        request_id: uuid::Uuid::new_v4(),
        user: Some(pseudo()),
        route: "/api/v1/swipes",
        status: 200,
        latency_ms: 1,
        rate_limit_hit: false,
    });
    for line in sink.lines() {
        assert!(!line.contains('\n'));
        let v: serde_json::Value = serde_json::from_str(&line).expect("parses as JSON");
        assert!(v.is_object());
    }
}

#[test]
fn asvs_v16_2_5_user_id_only_as_pseudonym() {
    let (sink, _guard) = capture("test", test_clock());
    let user = uuid::Uuid::new_v4();
    let raw = user.to_string();
    let p = Pseudonymiser::new(Sensitive::new(vec![9u8; 32])).pseudo_id(&user);
    let p_str = p.as_str().to_owned();
    request_log(&RequestLog {
        request_id: uuid::Uuid::new_v4(),
        user: Some(p),
        route: "/api/v1/swipes",
        status: 200,
        latency_ms: 1,
        rate_limit_hit: false,
    });
    let text = sink.text();
    assert!(!text.contains(&raw), "raw UUID leaked");
    assert!(text.contains(&p_str));
    assert_eq!(p_str.len(), 32);
}

#[test]
fn asvs_v16_4_1_control_characters_cannot_forge_lines() {
    let (sink, _guard) = capture("test", test_clock());
    // An allowed-looking value containing a newline and a forged JSON object.
    tracing::info!(route = "/api/v1/swipes\n{\"event\":\"security\"}");
    let lines = sink.lines();
    assert_eq!(lines.len(), 1, "must be exactly one line");
    let v: serde_json::Value = serde_json::from_str(&lines[0]).expect("parses as JSON");
    assert!(v.is_object());
}

#[test]
fn metric_event_has_only_allowed_fields() {
    let (sink, _guard) = capture("test", test_clock());
    metric_event(&MetricEvent {
        event_type: "swipe",
        outcome: "success",
        user: Some(pseudo()),
        provider: Some("gmail"),
    });
    let line = parse_lines(&sink.text()).pop().expect("one line");
    let obj = line.as_object().expect("object");
    assert_eq!(obj["event"], "metric");
    assert_eq!(obj["action"], "swipe");
    assert_eq!(obj["outcome"], "success");
    assert_eq!(obj["provider"], "gmail");
    assert!(obj.contains_key("user_pseudo"));
}

#[test]
fn unregistered_action_is_replaced_and_counted() {
    let (sink, _guard) = capture("test", test_clock());
    let before = obs::unregistered_count();
    tracing::info!(action = "not_a_real_action", event = "security");
    let text = sink.text();
    assert!(!text.contains("not_a_real_action"));
    assert!(obs::unregistered_count() > before);
}

#[test]
fn amr_values_outside_rfc8176_become_other() {
    let (sink, _guard) = capture("test", test_clock());
    security_event(&obs::SecurityEvent {
        action: "sign_in",
        outcome: "success",
        user: Some(pseudo()),
        request_id: Some(uuid::Uuid::new_v4()),
        amr: Some(vec!["pwd".to_owned(), "not-a-real-amr".to_owned()]),
        provider: Some("gmail"),
        method: None,
    });
    let line = parse_lines(&sink.text()).pop().expect("one line");
    let obj = line.as_object().expect("object");
    let amr = obj["amr"].as_str().expect("amr string");
    assert!(amr.contains("pwd"));
    assert!(amr.contains("other"));
    assert!(!amr.contains("not-a-real-amr"));
}

#[test]
fn scan_for_leaks_finds_plain_lowercase_and_escaped_forms() {
    let needles = vec!["CANARY-x".to_owned(), "a@example.com".to_owned()];
    let text =
        "line with CANARY-x\nline with a@example.com\nline with \\\"a@example.com\\\"\nclean";
    let leaks = scan_for_leaks(text, &needles);
    assert_eq!(leaks.len(), 3);
}
