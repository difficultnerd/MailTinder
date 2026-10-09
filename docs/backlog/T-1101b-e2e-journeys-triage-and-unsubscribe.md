# T-1101b: End-to-end helpers and the sessions journey

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M11 | sonnet | about 400 lines of code plus tests | T-1003, T-1006a, T-1101a, T-1112c |

**Read only these spec sections:** S10 sections 3.3 (journey list), 6.2 (one-click route group), 6.3 (rows "Reject queues job", "Undo before due time", "Every outcome recorded") and 8 (events per journey) (`docs/specs/S10-test-strategy.md`); S2 AU-07 AC1, FD-02 AC1 and AC2, SW-04 AC2, SW-05 AC2, UN-01 AC1 and AC3, UN-02 AC1. Nothing else is needed.

## Goal

Shared helpers for the browser journeys plus the first journey on the T-1101a harness: signing in again ends the earlier session. (Journeys 3 to 6 were split into T-1101d, T-1101e and T-1101f on 2026-10-09.)

## Files

| Action | Path | What |
| --- | --- | --- |
| Create | `backend/crates/e2e/tests/sessions.rs` | Journey 2 |
| Change | `backend/crates/e2e/src/lib.rs` | Helpers below |

## Types and signatures

```rust
pub struct Testbed { /* reqwest client to unsub-testbed */ }
impl Testbed { pub async fn received(&self, route: &str) -> Result<Vec<RecordedRequest>, E2eError>; }
pub struct RecordedRequest { pub method: String, pub headers: Vec<(String, String)>, pub body: Vec<u8> }

pub struct EventLog { /* reads target/e2e-logs/*.jsonl */ }
impl EventLog {
    pub fn since_mark(&self, mark: LogMark) -> Result<Vec<MetricEvent>, E2eError>;
    pub fn mark(&self) -> LogMark;   // byte offsets per file, so each journey reads only its own lines
}
pub struct MetricEvent { pub event_type: String, pub outcome: Option<String> } // field names from T-307's schema

/// Ends every journey: `save_browser_dump` then `assert_no_csp_violation`. Journeys call it explicitly as their last statement (async work cannot run reliably in `Drop`, so there is no Drop-based variant). `finish_journey` is defined HERE with a **stub body** that only records that it was called (a `JOURNEYS_FINISHED` marker file per test name under `target/e2e-logs/`); T-1101c replaces the stub with the real `save_browser_dump` and `assert_no_csp_violation` and adds a check in `zz_leak_scan.rs` that every journey test has a marker, so a journey that omits the call fails the scan.
pub async fn finish_journey(ui: Ui) -> Result<(), E2eError>;

/// Signs a seeded account in through the UI and returns a ready Ui on the Feed.
pub async fn signed_in_user(stack: &Stack, sub: &str, email: &str, fixtures: &[&str]) -> Result<Ui, E2eError>;
```

## Algorithm

All tests are `#[ignore = "run by scripts/e2e.sh"]`, use `example.com` accounts and corpus fixtures from T-204 only.

0. **Helpers.** Implement the helpers above, including `finish_journey` (a stub in this task: records the call; T-1101c makes it call `save_browser_dump` and `assert_no_csp_violation`). The sessions journey calls it.
1. **Journey 2, sign in again** (AU-07 AC1): sign in as user A in browser session 1; open browser session 2 and sign in as the same account; in session 1 tap "Keep" (or pull to refresh): the app gets `401`, wipes and shows the Sign-in default ("Continue with Google" visible, no card text).

## Acceptance criteria

| ID | Behaviour (one line) |
| --- | --- |
| AU-07 AC1 | Signing in again ends the earlier session; its next request is refused and it shows Sign-in |

## Tests that must pass

- `au_07_ac1_e2e_second_sign_in_ends_first_session` (end to end)

## Edge cases and traps

- Advance the virtual clock through `TestControl`; never `sleep` for minutes. Poll with a short timeout only for asynchronous delivery.
- The testbed listens on loopback; `unsub` runs with the test egress policy that allows exactly the testbed socket (S10 6.2). Do not loosen the production policy.
- Each journey seeds its own accounts with unique `sub` values so tests do not see each other's mail.
- Read events only after the mark, so earlier journeys' lines do not count.
- No real hosts, no real mail.

- Events (S10 8): journey 2 emits no metric event; assert none appears after the mark.

## Out of scope

- Journeys 3 to 6 (T-1101d, T-1101e, T-1101f); Needs Attention, disconnect, delete account, leak and CSP scans (T-1101c).

## Done when

- The tests above pass in `scripts/e2e.sh` and every required check is green (S10 10.1).
- Definition of done in S10 10.4.
