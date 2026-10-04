# T-307: Structured logging with redaction

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M3 | strong | about 450 lines of code plus about 300 lines of tests | T-001, T-201a |

**Read only these spec sections:** S5 "Logs and telemetry" and LOG-1 (`docs/specs/S5-data-inventory.md`); S6 section 3 row T12 and section 7 (`docs/specs/S6-security.md`); S10 7.3 and 8 (first paragraph and "Tests for the measures themselves") (`docs/specs/S10-test-strategy.md`); S2 XC-01 (`docs/specs/S2-v1-acceptance-criteria.md`); ASVS register rows V16.2.1, V16.2.2, V16.2.4, V16.2.5, V16.4.1, V16.5.1 (`docs/security/asvs-l2-register.md`); `docs/backlog/T-003-privacy-layer.md` "Edge cases and traps" (how the Semgrep rules read log calls). Nothing else is needed.

## Goal

The `obs` crate becomes the only way anything reaches a log line. A `tracing` layer writes one JSON object per event to stdout (Cloud Logging's structured format) and keeps only the S5 allowlisted fields, each with a validated shape; every other field, and every free-text message, is dropped. Typed helpers build request logs, security events and metric events. User IDs appear only as an HMAC pseudonym. A capture sink and a leak scanner let any test prove that no canary, address or URL reached the log (LOG-1, XC-01).

## Files

| Action | Path | What |
| --- | --- | --- |
| Keep | `backend/crates/obs/src/sensitive.rs` | from T-201a (create it with T-201a's exact API if absent) |
| Create | `backend/crates/obs/src/pseudo.rs` | `Pseudonymiser`, `PseudoId` |
| Create | `backend/crates/obs/src/registry.rs` | closed lists: events, actions, outcomes, operation names, AMR values; HTTP route registration |
| Create | `backend/crates/obs/src/layer.rs` | `AllowlistJsonLayer` (the redacting `tracing` layer) |
| Create | `backend/crates/obs/src/events.rs` | `request_log`, `security_event`, `metric_event`, `op_log` and their structs |
| Create | `backend/crates/obs/src/sink.rs` | `LogSink` trait, `StdoutSink`, `CaptureSink` |
| Create | `backend/crates/obs/src/init.rs` | `init(service, sink)`, panic hook |
| Create | `backend/crates/obs/src/scan.rs` | `scan_for_leaks` |
| Change | `backend/crates/obs/src/lib.rs` | modules and re-exports |
| Change | `backend/crates/obs/Cargo.toml` | deps `tracing`, `tracing-subscriber` (features `registry`, `std`; no `fmt` ansi), `serde_json`, `time`, `uuid`, `hmac`, `sha2`, `hex` |
| Create | `backend/crates/obs/tests/redaction.rs` | the tests below |

## Types and signatures

```rust
// pseudo.rs
#[derive(Clone, PartialEq, Eq, Hash)] pub struct PseudoId(String);   // 32 lower-case hex chars
impl PseudoId { pub fn as_str(&self) -> &str; }
impl std::fmt::Debug for PseudoId { /* prints the value: it is C1 */ }
pub struct Pseudonymiser { key: Sensitive<Vec<u8>> }
impl Pseudonymiser {
    pub fn new(key: Sensitive<Vec<u8>>) -> Self;                       // LogPseudonymHmacKey from Secrets (T-305)
    /// First 16 bytes of HMAC-SHA-256(key, "mt.log.user.v1" || user UUID bytes), lower-case hex.
    pub fn pseudo_id(&self, user: &uuid::Uuid) -> PseudoId;
}

// registry.rs
pub const EVENTS: &[&str] = &["request", "security", "metric", "op", "panic"];
pub const ACTIONS: &[&str];      // S6 7 list as snake_case: "sign_in", "step_up", "session_end", "mailbox_link",
                                 // "mailbox_unlink", "invite_create", "invite_revoke", "invite_use", "invite_request_approve",
                                 // "invite_request_decline", "experiments_consent", "kill_switch_change", "snapshot_save",
                                 // "snapshot_delete", "admin_action", "authz_failure", "csrf_failure", "rate_limit_hit",
                                 // "unsub_job_outcome", "account_delete", "sign_in_required", "egress_tls_failure",
                                 // "egress_refused", "token_revoke"; metric types: "swipe", "undo", "unsub_outcome",
                                 // "delivery_check_outcome", "undo_failed", "unsub_after_undo", "history_missing"
pub const OUTCOMES: &[&str];     // "success", "failure", "refused", "expired", "replaced", "admin_ended", "signed_out",
                                 // "sent", "cancelled", "needs_attention", "failed", "tls_failure", "host_refused",
                                 // "address_refused", "redirected", "timed_out", "rejected", "revoke_failed", "rate_limited",
                                 // and the S7 section 4 error codes
pub const OPS: &[&str];          // non-HTTP operation names: "gmail.messages.list", "gmail.messages.get",
                                 // "drive.files.update", "egress.one_click", "egress.call", "tasks.create", "kms.decrypt", ...
pub const AMR_VALUES: &[&str] = &["pwd", "mfa", "otp", "hwk", "swk", "sms", "user", "pin", "fpt", "face", "kba"]; // RFC 8176
pub const PROVIDERS: &[&str] = &["gmail", "graph"];
pub const JOB_METHODS: &[&str] = &["one_click", "mailto"];
/// Called once at start-up by each service with its router's route templates, e.g. "/api/v1/swipes".
pub fn register_http_routes(routes: &'static [&'static str]);

// events.rs
pub struct RequestLog { pub request_id: uuid::Uuid, pub user: Option<PseudoId>, pub route: &'static str,
                        pub status: u16, pub latency_ms: u64, pub rate_limit_hit: bool }
pub struct SecurityEvent { pub action: &'static str, pub outcome: &'static str, pub user: Option<PseudoId>,
                           pub request_id: Option<uuid::Uuid>, pub amr: Option<Vec<String>>,
                           pub provider: Option<&'static str>, pub method: Option<&'static str> }
pub struct MetricEvent { pub event_type: &'static str, pub outcome: &'static str, pub user: Option<PseudoId>,
                         pub provider: Option<&'static str> }   // S10 8: event type, outcome, pseudo user, provider, time
pub struct OpLog { pub op: &'static str, pub outcome: &'static str, pub status: Option<u16>, pub latency_ms: Option<u64> }
pub fn request_log(e: &RequestLog);
pub fn security_event(e: &SecurityEvent);   // severity NOTICE
pub fn metric_event(e: &MetricEvent);
pub fn op_log(e: &OpLog);                   // severity DEBUG for 2xx, WARNING otherwise

// sink.rs
pub trait LogSink: Send + Sync + 'static { fn write_line(&self, line: &str); }
pub struct StdoutSink;                                   // locked stdout, one line, flushed
#[derive(Clone, Default)] pub struct CaptureSink { lines: Arc<Mutex<Vec<String>>> }
impl CaptureSink { pub fn lines(&self) -> Vec<String>; pub fn text(&self) -> String; }

// init.rs
pub fn init(service: &'static str, sink: Arc<dyn LogSink>) -> Result<(), ObsError>; // global, once per process
/// For tests: a scoped subscriber on the current thread.
pub fn capture(service: &'static str) -> (CaptureSink, tracing::subscriber::DefaultGuard);
pub fn unregistered_count() -> u64;          // values replaced because they were not in a registry

// scan.rs
pub struct Leak { pub line_no: usize, pub needle_index: usize }  // never echo the needle
pub fn scan_for_leaks(text: &str, needles: &[String]) -> Vec<Leak>;
```

Output line (one JSON object, no other text), keys in this order:

```json
{"time":"2026-10-05T00:00:00.000000Z","severity":"NOTICE","service":"unsub","event":"security",
 "action":"unsub_job_outcome","outcome":"sent","user_pseudo":"3f2a...","request_id":"6f1c...","method":"one_click"}
```

## Algorithm

1. Allowed keys and their checks (S5 "Field allowed" plus the structural keys every line needs):
   - Structural: `time` (from the event's timestamp, RFC 3339 UTC with microseconds), `severity` (from the level: TRACE and DEBUG `DEBUG`, INFO `INFO`, the `security_event` helper `NOTICE`, WARN `WARNING`, ERROR `ERROR`), `service` (the `&'static str` given to `init`), `event` (must be in `EVENTS`).
   - S5 fields: `request_id` (parses as a UUID), `user_pseudo` (exactly 32 chars of `[0-9a-f]`), `route` (in the registered HTTP routes or `OPS`), `status` (integer 100 to 599), `latency_ms` (unsigned integer), `action` (in `ACTIONS`), `outcome` (in `OUTCOMES`), `rate_limit_hit` (bool).
   - Additions this task needs (reported for S5): `amr` (each value in `AMR_VALUES`, others become `other`; S6 7 asks for it on sign-in events), `provider` (in `PROVIDERS`; S10 8 metric events), `method` (in `JOB_METHODS`).
2. `AllowlistJsonLayer::on_event`: visit every field with a `tracing::field::Visit` that only records `&str`, `u64`, `i64`, `bool` values for allowed keys. A value that fails its check is replaced by `"[unregistered]"` (registry keys) or dropped (shape keys), and `unregistered_count` increments. Every other key is dropped, including `message` (so `info!("user {}", x)` loses its text). Count dropped keys in `dropped_fields` (a number) when non-zero, so a reviewer can find stray log calls.
3. Serialise with `serde_json` (it escapes control characters, quotes and backslashes; ASVS V16.4.1) and write exactly one line to the sink. Never write partial lines.
4. Helpers in `events.rs` call `tracing::event!` with the exact key names above, so typed callers and the layer agree. The pseudonymous user is passed as `user_pseudo = pseudo.as_str()`.
5. `init`: build `tracing_subscriber::registry().with(AllowlistJsonLayer::new(service, sink))`, set it global; set a panic hook that writes one line `{"event":"panic","severity":"CRITICAL",...}` with no payload and no location, then calls the previous default hook only in debug builds `[DEFAULT]` (release builds never print the panic message, which can hold data).
6. `Pseudonymiser`: HMAC-SHA-256 with domain separation label `mt.log.user.v1`; same output as `ports::store::UserPseudoId` expects (T-201b), so `classifier_eval` and logs use one pseudonym.
7. `scan_for_leaks(text, needles)`: case-sensitive substring search for each needle, plus the needle lower-cased; also search the JSON-escaped form of each needle. Return positions only.
8. LOG-1 integration (later tasks): service integration tests create a `capture`, run, then call `scan_for_leaks(capture.text(), &corpus.all_canaries() + all_addresses() + all_urls())` (T-204) and assert empty. This task provides the tools and the unit-level proof below.

## Acceptance criteria

| ID | Behaviour (one line) |
| --- | --- |
| XC-01 | No body, subject, snippet, address or URL can appear in log output; only allowlisted fields pass |
| LOG-1 | Canary values from the fixture corpus never reach captured log output (unit-level proof here; every integration suite reuses the scanner) |
| V16.2.1 | Each line has time, service, event, request ID, pseudonymous user, route and outcome where relevant |
| V16.2.4 | Lines are single JSON objects in Cloud Logging's structured format |
| V16.2.5 | Only allowlisted fields are logged; user IDs only as an HMAC pseudonym; no tokens |
| V16.4.1 | Log output is JSON-encoded; control characters and newlines cannot forge lines |

## Tests that must pass

All in `obs/tests/redaction.rs`, each using `capture()`.

- `xc_01_unknown_fields_and_messages_are_dropped` (unit: `info!(email = "a@example.com", url = "https://x.example.com", "hello {}", "CANARY-x")` leaves none of them; `dropped_fields` counts them).
- `xc_01_helpers_emit_only_allowlisted_keys` (unit: every helper's line has a key set within the allowlist).
- `log_1_canaries_never_reach_log_output` (unit: push a list of canaries, example.com addresses and URLs through every path: free text, unknown keys, allowed keys with bad values (`action = "CANARY..."`, `route = "/api/v1/messages/CANARY"`, `user_pseudo = "a@example.com"`), and `Debug` of `Sensitive`; `scan_for_leaks` finds nothing).
- `panic_hook_writes_no_payload` (unit, its own test binary `obs/tests/panic_hook.rs` because it calls the global `init` with a `CaptureSink`: a panic whose message holds a canary produces one `panic` line without it).
- `asvs_v16_2_1_security_event_has_required_metadata` (unit).
- `asvs_v16_2_4_each_line_is_one_json_object` (unit: every captured line parses with `serde_json` and has no embedded newline).
- `asvs_v16_2_5_user_id_only_as_pseudonym` (unit: the raw UUID string never appears; the pseudonym is 32 hex chars and stable for one key, different for another key).
- `asvs_v16_4_1_control_characters_cannot_forge_lines` (unit: an allowed-looking value containing `\n{"event":"security"}` is refused or escaped; line count stays one).
- `metric_event_has_only_allowed_fields` (unit, S10 8).
- `unregistered_action_is_replaced_and_counted` (unit).
- `amr_values_outside_rfc8176_become_other` (unit).
- `scan_for_leaks_finds_plain_lowercase_and_escaped_forms` (unit).

## Edge cases and traps

- `tracing` events can carry `Debug`-formatted values (`?x`); the visitor must ignore `record_debug` for every key. Never call `format!("{:?}")` on a field value to "check" it.
- Do not use `tracing_subscriber::fmt` anywhere in services; it prints every field.
- Registries are closed on purpose. A task that logs a new action, outcome or operation adds its name to `registry.rs` in the same pull request; reviewers check names carry no data.
- HTTP routes must be templates (`/api/v1/mailboxes/{id}`), never raw paths; `register_http_routes` receives the router's templates (T-500).
- The pseudonym key rotates yearly (S5); pseudonyms change with it. Do not try to keep them stable.
- `capture()` uses a thread-local default subscriber. Tests that spawn tasks on other threads must use a `current_thread` Tokio runtime or the global `init` with a `CaptureSink`.
- `println!`, `eprintln!` and `dbg!` are banned by Clippy (T-003). `StdoutSink` writes through `std::io::Write` on `stdout().lock()`.
- Semgrep's privacy log rule matches log calls that mention words like `token` or `email` even in field names; the helpers here avoid those words, and so must callers.
- Error Reporting reads stack traces from logs; never log a backtrace or an error's `Display` text from a provider (it can hold addresses). Log an outcome code instead (ASVS V16.5.1 for responses is T-500).

## Out of scope

- Which events each feature emits (their own tasks add calls and registry names).
- Log bucket lock, retention and access (T-1102). Alerts (T-1107).
- The service integration leak runs over full suites (T-500 onwards, T-1101).

## Security review checklist

- The layer is an allowlist: unknown keys and the `message` field are always dropped; there is no code path that writes a caller-supplied string except values from closed registries or strictly validated shapes (UUID, 32-hex pseudonym, integers, bools).
- Allowed keys equal S5's list plus `time`, `severity`, `service`, `event`, `amr`, `provider`, `method`; the additions are reported for S5.
- User IDs are pseudonymised with HMAC-SHA-256 under the Secret Manager key with a domain-separation label; raw IDs, emails, tokens, cookies, message IDs, URLs and hosts never appear.
- Output is one JSON object per line via `serde_json`; injection attempts cannot create new lines or keys.
- The panic hook writes no payload in release builds.
- `scan_for_leaks` is used by the LOG-1 test here and is ready for integration suites.
- No `tracing_subscriber::fmt`, `println!` or `eprintln!` anywhere in production crates.

## Done when

- The tests above pass and every required check is green (S10 10.1).
- Definition of done in S10 10.4.
- A strong-model review has signed off the checklist above in the pull request.
