# T-707: Delivery check and confirmed unsubscribes

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M7 | sonnet | about 300 lines of code plus tests | T-602b, T-609, T-706 |

**Read only these spec sections:** S2 UN-06 AC1, AC2 and ST-02 AC1 (`docs/specs/S2-v1-acceptance-criteria.md`), S3 "User app folder file" table rows `HistoryEntry` and `PendingDeliveryCheck`, and "Rule matching and counting" (`docs/specs/S3-domain-model.md`), S7 5.4 the bullet starting "This is a `POST` because" and S7 5.8 reason `unsubscribe_ignored` (`docs/specs/S7-api-contract.md`), S5 app folder row "Pending delivery checks" (`docs/specs/S5-data-inventory.md`), S10 6.3 row "Delivery check and stats" and S10 8 row "Unsubscribe success rate" (`docs/specs/S10-test-strategy.md`). Nothing else is needed.

## Goal

After an unsubscribe is sent, the api watches for more mail from that list each time the Feed loads. Mail that arrives more than 5 Australian business days after the send is trashed by the rule (as before) and raises a Needs Attention item "Mail still arriving after unsubscribe". Separately, an unsubscribe counts as "confirmed working" in Stats once 14 days pass with no mail from the list. Both run inside API-FEED-1, using the user state file.

## Files

| Action | Path | What |
| --- | --- | --- |
| Create | `backend/crates/domain/src/delivery.rs` | `BusinessCalendar`, `DeliveryCheck` logic (pure) |
| Change | `backend/crates/domain/src/lib.rs` | `pub mod delivery;` |
| Change | `backend/crates/domain/src/user_state.rs` | `PendingDeliveryCheck` gains `mail_seen` and `ignored_raised` (see Types) |
| Create | `backend/crates/api/src/services/delivery_check.rs` | Feed-load hook |
| Change | `backend/crates/api/src/services/feed.rs` | Call the hook after `apply_rules` and after the job outcome append |
| Create | `backend/crates/api/config/au_holidays.toml` | National public holidays `[DEFAULT]` list |
| Create | `backend/crates/api/tests/delivery_check.rs` | Service integration tests |

## Types and signatures

```rust
// domain/src/delivery.rs
pub const GRACE_BUSINESS_DAYS: u32 = 5;       // S2 UN-06 AC1
pub const CONFIRM_DAYS: i64 = 14;             // S2 ST-02 AC1
pub const CHECK_LIFETIME_DAYS: i64 = 30;      // [DEFAULT] how long a check keeps watching for late mail
pub const AU_OFFSET_HOURS: i8 = 10;           // [DEFAULT] AEST; daylight saving ignored (one hour at most)

pub struct BusinessCalendar { holidays: BTreeSet<time::Date> }
impl BusinessCalendar {
    pub fn new(holidays: BTreeSet<time::Date>) -> Self;
    pub fn is_business_day(&self, d: time::Date) -> bool;     // Monday to Friday and not a holiday
    /// Moves forward n business days, keeping the local (UTC+10) time of day. Starting on a
    /// non-business day counts from the next business day's same time.
    pub fn add_business_days(&self, from: OffsetDateTime, n: u32) -> OffsetDateTime;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MailVerdict { WithinGrace, Ignored }
/// Pure: is this message late? `received_at` is the message's own date, not the fetch time.
pub fn judge_mail(cal: &BusinessCalendar, unsubscribed_at: OffsetDateTime, received_at: OffsetDateTime) -> MailVerdict;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CheckEnd { Confirmed, NotConfirmed, Keep }
/// Pure: what to do with a check at `now`.
/// Confirmed when now >= unsubscribed_at + 14 days, no mail seen, not yet counted.
pub fn evaluate(check: &PendingDeliveryCheck, now: OffsetDateTime) -> CheckEnd;

// domain/src/user_state.rs (T-602b type, two fields added; serde default false keeps old files readable)
pub struct PendingDeliveryCheck {
    pub sender_key: String, pub list_id: Option<String>, pub mailbox_id: MailboxId, pub unsubscribed_at: OffsetDateTime,
    #[serde(default)] pub mail_seen: bool,        // any mail from the list after unsubscribed_at
    #[serde(default)] pub confirm_counted: bool,  // Totals.unsubscribes_confirmed already incremented
}

// api/src/services/delivery_check.rs
pub struct DeliveryHookInput<'a> { pub list_mail: &'a [ListMail] }    // messages the rules matched this load
pub struct ListMail { pub sender_key: String, pub list_id: Option<String>, pub mailbox_id: MailboxId,
                      pub received_at: OffsetDateTime, pub sender_display: String }
/// Mutates the user state inside the same UserStateStore::update closure as the Feed; returns the
/// Needs Attention items to raise after the state write succeeds.
pub fn apply_delivery_checks(state: &mut UserState, cal: &BusinessCalendar, now: OffsetDateTime,
                             input: &DeliveryHookInput) -> Vec<IgnoredUnsubscribe>;
pub struct IgnoredUnsubscribe { pub mailbox_id: MailboxId, pub sender_display: String }
```

## Algorithm

Creating checks (inside the Feed's job-outcome append, T-609): when an outcome `sent` (any `Sent` code, including `Batched`) is appended to History, push `PendingDeliveryCheck { sender_key, list_id, mailbox_id, unsubscribed_at: outcome.at, mail_seen: false, confirm_counted: false }` using the `PendingUnsubscribe` entry for that job (T-602b holds sender key and List-Id by job ID). Skip if a check with the same `(sender_key, list_id)` already exists (batching). Also increment `Totals.senders_unsubscribed` there if T-609 does not.

On every Feed load, inside the same `UserStateStore::update` that writes the Feed position:

1. Collect `ListMail` for every message `apply_rules` trashed with a `reject_list` rule, plus every card about to be shown whose `(sender_key, list_id)` matches a check (a user may have switched the rule off; the check still sees the mail).
2. For each `ListMail`, find checks with the same `sender_key` and `list_id` (both equal; a `None` List-Id matches only `None`) and `received_at > unsubscribed_at`:
   1. Set `mail_seen = true`.
   2. If `judge_mail(...) == Ignored`: emit `IgnoredUnsubscribe` once per check and remove the check (UN-06 AC1).
3. Mail with `received_at <= unsubscribed_at` is ignored (it was already on its way before the send).
4. For each remaining check, `evaluate(check, now)`:
   - `Confirmed`: `Totals.unsubscribes_confirmed += 1`, set `confirm_counted = true`, keep the check (it still watches for late mail until `CHECK_LIFETIME_DAYS`).
   - `NotConfirmed` (14 days passed with `mail_seen`): keep the check, never count it.
   - Remove any check older than `CHECK_LIFETIME_DAYS`.
5. After the state write succeeds, for each `IgnoredUnsubscribe` call `raise_item(reason: UnsubscribeIgnored, link: None)` and emit metric `delivery_check_outcome` with outcome `ignored`; each `Confirmed` emits `delivery_check_outcome` `confirmed`.
6. Order matters: process the mail (steps 1 to 3) before evaluating confirmation (step 4), so mail from day 10 seen on day 20 blocks confirmation.

`judge_mail`: `deadline = cal.add_business_days(unsubscribed_at, GRACE_BUSINESS_DAYS)`; `received_at > deadline` gives `Ignored`, else `WithinGrace` ("more than 5 business days later"; exactly at the deadline is within grace).

`evaluate`: if `confirm_counted` gives `Keep`; if `now >= unsubscribed_at + 14 days` then `mail_seen` gives `NotConfirmed` else `Confirmed`; otherwise `Keep`. Mail inside the grace period still blocks confirmation `[DEFAULT: S10 6.3 says "no mail from the list arrives within 14 days"]`.

Holidays: load `config/au_holidays.toml` at start-up into `BusinessCalendar` `[DEFAULT]` with national days only (James to confirm):
2026-01-01, 2026-01-26, 2026-04-03, 2026-04-06, 2026-04-25, 2026-12-25, 2026-12-28, 2027-01-01, 2027-01-26, 2027-03-26, 2027-03-29, 2027-04-26, 2027-12-27, 2027-12-28. Tests inject their own calendar.

## Acceptance criteria

| ID | Behaviour (one line) |
| --- | --- |
| UN-06 AC1 | List mail more than 5 business days after the send is trashed by the rule and raises Needs Attention |
| UN-06 AC2 | The delivery check and the 14-day Stats measure are independent |
| ST-02 AC1 | An unsubscribe counts as confirmed only after 14 days with no mail from the list |

## Tests that must pass

- `un_06_ac1_mail_after_5_business_days_raises_item` (service integration: send Monday 2026-10-05 09:00 AEST, mail Monday 2026-10-12 09:01 AEST; item `unsubscribe_ignored`)
- `un_06_ac1_mail_at_exactly_5_business_days_no_item` (unit on `judge_mail`)
- `un_06_ac1_weekend_and_holiday_skipped` (unit: send Thursday 2026-12-24 10:00 AEST with holidays 2026-12-25, 2026-12-28, 2027-01-01; deadline is Tuesday 2027-01-05 10:00 AEST)
- `un_06_ac1_mail_trashed_by_rule` (service integration: the message is in Trash and History has `trashed_by_rule`)
- `un_06_ac1_one_item_per_check` (service integration: three late messages, one item)
- `un_06_ac2_day_10_mail_raises_item_never_confirmed` (service integration: mail on day 10; item raised; day 15 Feed load leaves `unsubscribes_confirmed` unchanged)
- `st_02_ac1_confirmed_after_14_days_without_mail` (service integration)
- `st_02_ac1_not_confirmed_at_13_days_23_hours` (service integration)
- `st_02_ac1_mail_within_grace_blocks_confirmation` (service integration: mail on day 2, no item, never confirmed)
- `st_02_ac1_confirmed_counted_once` (service integration: two Feed loads after day 14)
- `delivery_old_mail_before_send_ignored` (unit)
- `delivery_check_removed_after_lifetime` (unit)
- `business_calendar_add_days_table` (unit: Monday +5 = next Monday; Friday +1 = Monday; Saturday +1 = Tuesday same time; across 2026-12-25 and 2026-12-28)

## Edge cases and traps

- Judge by the message's `internal_date`, never by when the Feed fetched it.
- The list key is `(sender_key, list_id)` after relay unwrapping (S3 `sender_key`); compare normalised values only.
- Do not raise the item before the state write succeeds; a lost write would raise it twice on the next load.
- Date arithmetic uses `time` with a fixed `UtcOffset::from_hms(10, 0, 0)`; do not pull in a time zone database.
- The Clock port supplies `now`; tests advance it day by day.
- Nothing about the list goes to the server database except the Needs Attention item (encrypted). Checks live in the user state file only.
- The sender display in the item comes from `ListMail.sender_display` (plain text), never the address.

## Out of scope

- Stats endpoint output: T-802 reads `Totals.unsubscribes_confirmed`.
- Rule matching and the History entry for trash by rule: T-609.

## Done when

- The tests above pass and every required check is green (S10 10.1).
- Definition of done in S10 10.4.
- The PR notes the two `PendingDeliveryCheck` fields added to the T-602b type.
