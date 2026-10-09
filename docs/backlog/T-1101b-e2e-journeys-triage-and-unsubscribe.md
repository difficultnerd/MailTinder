# T-1101b: End-to-end journeys: sessions, mixed Feed, reject, unsubscribe and filing

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M11 | sonnet | about 400 lines of code plus tests | T-1003, T-1006a, T-1101a, T-1112c |

**Read only these spec sections:** S10 sections 3.3 (journey list), 6.2 (one-click route group), 6.3 (rows "Reject queues job", "Undo before due time", "Every outcome recorded") and 8 (events per journey) (`docs/specs/S10-test-strategy.md`); S2 AU-07 AC1, FD-02 AC1 and AC2, SW-04 AC2, SW-05 AC2, UN-01 AC1 and AC3, UN-02 AC1. Nothing else is needed.

## Goal

Five browser journeys on the T-1101a harness: signing in again ends the earlier session, a mixed Feed across two mailboxes, reject with unsubscribe then undo, reject and let the unsubscribe run, and filing a message. Each journey also checks the metric events it should emit.

## Files

| Action | Path | What |
| --- | --- | --- |
| Create | `backend/crates/e2e/tests/sessions.rs` | Journey 2 |
| Create | `backend/crates/e2e/tests/mixed_feed.rs` | Journey 3 |
| Create | `backend/crates/e2e/tests/reject_undo.rs` | Journey 4 |
| Create | `backend/crates/e2e/tests/reject_unsubscribe.rs` | Journey 5 |
| Create | `backend/crates/e2e/tests/file.rs` | Journey 6 |
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

/// Signs a seeded account in through the UI and returns a ready Ui on the Feed.
pub async fn signed_in_user(stack: &Stack, sub: &str, email: &str, fixtures: &[&str]) -> Result<Ui, E2eError>;
```

## Algorithm

All tests are `#[ignore = "run by scripts/e2e.sh"]`, use `example.com` accounts and corpus fixtures from T-204 only.

1. **Journey 2, sign in again** (AU-07 AC1): sign in as user A in browser session 1; open browser session 2 and sign in as the same account; in session 1 tap "Keep" (or pull to refresh): the app gets `401`, wipes and shows the Sign-in default ("Continue with Google" visible, no card text).
2. **Journey 3, mixed Feed** (FD-02 AC1, AC2): seed mailbox A with fixtures at times t1 and t3 and mailbox B at t2 (fake-google control). Sign in with A; Settings, Connected accounts, "Add Gmail"; Confirm it's you appears: switch to the popup, fake-google re-authenticates A; back in the main window the link starts; fake-google selects B; return to Connected accounts. Go to the Feed and read three cards in turn by tapping "Keep": order t3 (A), t2 (B), t1 (A), each showing its mailbox address.
3. **Journey 4, reject then undo** (SW-05 AC2): seed one one-click list message (DKIM covers both headers) pointing at the testbed's one-click 200 route. Tap "Reject"; wait for "Trashed. Unsubscribing in 5 minutes."; tap "Undo"; wait for the card again. `advance_clock(6 minutes)`; then the testbed has received zero requests on that route, and fake-google shows the message back in INBOX with its exact label set.
4. **Journey 5, reject and let it run** (UN-01 AC1, UN-02 AC1, UN-01 AC3): same seed; tap "Reject"; `advance_clock(6 minutes)`; poll the testbed up to 10 s: exactly one POST, body exactly `List-Unsubscribe=One-Click`, no `Cookie`, no `Authorization`. Pull to refresh the Feed (collects the outcome), open Settings, History, filter "Unsubscribes": one entry with outcome "Sent". Advance the clock again and refresh: still exactly one request (runs once).
5. **Journey 6, file** (SW-04 AC2): seed one message; tap "File"; tap "New category", type "Receipts", confirm; wait for "Filed under Receipts."; fake-google shows a label named "Receipts" on the message and no INBOX label.
6. **Events** (S10 8): each journey takes `EventLog::mark()` at start and asserts exactly the expected events at the end: journey 4 one `swipe` with action reject and one `undo`, and no `unsub_outcome`; journey 5 one `swipe` and one `unsub_outcome` with outcome `sent`; journey 6 one `swipe`. Event field names come from T-307; if they differ from `event_type` and `outcome`, use T-307's.

## Acceptance criteria

| ID | Behaviour (one line) |
| --- | --- |
| AU-07 AC1 | Signing in again ends the earlier session; its next request is refused and it shows Sign-in |
| FD-02 AC1 | The Feed interleaves two mailboxes by received time, newest first |
| FD-02 AC2 | Each card shows its mailbox |
| SW-05 AC2 | Undo before the due time cancels the job: no request ever reaches the testbed |
| UN-01 AC1 | A queued unsubscribe runs exactly once after its due time |
| UN-01 AC3 | The outcome appears in History after the next Feed load |
| UN-02 AC1 | The one-click POST has the fixed body and no cookies or credentials |
| SW-04 AC2 | Filing applies the label and the message leaves the inbox |

## Tests that must pass

- `au_07_ac1_e2e_second_sign_in_ends_first_session` (end to end)
- `fd_02_ac1_e2e_two_mailboxes_interleaved_newest_first` (end to end; also asserts FD-02 AC2 badges, name `fd_02_ac2_e2e_cards_show_mailbox` as a second test in the same file)
- `sw_05_ac2_e2e_undo_before_due_sends_nothing` (end to end)
- `un_01_ac1_e2e_unsubscribe_runs_once` (end to end)
- `un_02_ac1_e2e_one_click_post_is_exact` (end to end)
- `un_01_ac3_e2e_outcome_in_history_after_feed_load` (end to end)
- `sw_04_ac2_e2e_file_applies_label_and_leaves_inbox` (end to end)

## Edge cases and traps

- Advance the virtual clock through `TestControl`; never `sleep` for minutes. Poll with a short timeout only for asynchronous delivery.
- The testbed listens on loopback; `unsub` runs with the test egress policy that allows exactly the testbed socket (S10 6.2). Do not loosen the production policy.
- Each journey seeds its own accounts with unique `sub` values so tests do not see each other's mail.
- Read events only after the mark, so earlier journeys' lines do not count.
- No real hosts, no real mail.

## Out of scope

- Needs Attention, disconnect, delete account, leak and CSP scans (T-1101c).

## Done when

- The tests above pass in `scripts/e2e.sh` and every required check is green (S10 10.1).
- Definition of done in S10 10.4.
