# T-906a: Evaluation records

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M9 | sonnet | about 250 lines of code plus tests | T-305, T-904, T-905, T-907 |

**Read only these spec sections:** S2 CL-03 AC1 to AC3 (`docs/specs/S2-v1-acceptance-criteria.md`), S4 5.6 first two paragraphs (`docs/specs/S4-architecture.md`), S5 row `classifier_eval/{id}` and test EXP-1 (`docs/specs/S5-data-inventory.md`), S7 5.13 "How cards carry the bake-off" second paragraph and S7 5.5 `classification_token` bullet (`docs/specs/S7-api-contract.md`), S3 "Classification" bullet on swipe-to-label mapping (`docs/specs/S3-domain-model.md`), S10 9.2 rows BAKE-6 and EXP-1 (`docs/specs/S10-test-strategy.md`). Nothing else is needed.

Split note: the index's T-906 "Evaluation records and kill switches" is split into T-906a (this file) and T-906b (kill switches and admin switch endpoints).

## Goal

When a consenting user swipes a card whose token carries model predictions, the api writes exactly one `classifier_eval` record holding the header-rules result, both models' predictions, the swipe direction, undo status and time to swipe, plus the allowed buckets and version strings, and nothing that identifies the message or its content. Undo marks the same record undone. These records feed the bake-off report (T-908a).

## Files

| Action | Path | What |
| --- | --- | --- |
| Create | `backend/crates/api/src/classify/eval.rs` | `eval_id_for`, `build_eval_record`, `write_eval`, `mark_undone` |
| Change | `backend/crates/api/src/services/swipe.rs` (T-604) | Call `write_eval` after the swipe's provider change succeeds |
| Change | `backend/crates/api/src/services/undo.rs` (T-604) | Call `mark_undone` |
| Change | the undo token payload (T-604) | Add `eval_id: Option<Uuid>` |
| Create | `backend/crates/api/tests/eval_records.rs` | Service integration tests |

## Types and signatures

```rust
// api/src/classify/eval.rs
pub const EVAL_TTL_DAYS: i64 = 180;                        // S4 5.6 [TUNABLE]
/// Fixed namespace for eval IDs. Different from the job and rule namespaces so the same
/// (user, Idempotency-Key) never gives a job ID equal to an eval ID [DEFAULT].
pub const EVAL_NAMESPACE: Uuid = uuid::uuid!("6f0c8a52-3c1e-5d7a-9b1e-2a4c6e8f0a13");

pub fn eval_id_for(user: &UserId, idempotency_key: &Uuid) -> EvalId;   // UUID v5(EVAL_NAMESPACE, user bytes ++ key bytes)

pub fn build_eval_record(
    user_pseudo_id: UserPseudoId, payload: &ClassificationPayload, bakeoff: &BakeoffPayload,
    direction: SwipeDirection, now: OffsetDateTime, eval_id: EvalId,
) -> ClassifierEvalRecord;

/// Writes with Precondition::MustNotExist; AlreadyExists is success (retried swipe).
/// Returns None (writes nothing) when the direction is Down, the payload has no bakeoff part,
/// or the user's consent is no longer current.
pub async fn write_eval(app: &AppState, session: &AuthedSession, payload: &ClassificationPayload,
                        direction: SwipeDirection, idempotency_key: &Uuid) -> Result<Option<EvalId>, ApiError>;

/// Sets outcome.undone = true with Precondition::Matches; a missing record is fine.
pub async fn mark_undone(app: &AppState, eval_id: &EvalId) -> Result<(), ApiError>;
```

`ClassifierEvalRecord`, `ModelPrediction`, `EvalOutcome`, `SwipeDirection`, `UserPseudoId`, `EvalId` from T-201b; `ClassificationPayload`, `BakeoffPayload` from T-901; `consent_is_current` from T-902; the pseudonymiser from T-307.

## Algorithm

`write_eval` (after the provider action of API-SW-1 succeeded):

1. If `direction == Down` (skip): return `None` [DEFAULT: S10 BAKE-6 "down writes no label"; a skipped card that comes back is recorded when it is finally swiped, so one card gives at most one labelled record].
2. If `payload.bakeoff` is `None`: return `None` (the user was not consenting when the card was served).
3. Load the user; if `!consent_is_current(user)`: return `None` (opted out between serve and swipe; CL-02 AC3).
4. `eval_id = eval_id_for(user, idempotency_key)`.
5. Map the swipe action: keep is `Right`, reject is `Left`, file is `Up`.
6. `build_eval_record`:
   - `user_pseudo_id`: current pseudonymous ID;
   - `created_at = now`, `expires_at = now + 180 days`;
   - `header_rules = { class, score }` from `payload.header_rules`;
   - `gemini`, `jev` copied from the payload (a `Some` with `error_code` records a failure, CL-03 AC2);
   - `outcome = { direction, undone: false, time_to_swipe_ms: (now - payload.issued_at) in ms, clamped to 0..=u32::MAX }`;
   - `header_facts`, `provider`, `age_bucket`, `text_tokens_bucket`, `lang_is_english`, `input_version`, `question_version`, `price_version` copied from the payload.
7. `classifier_eval().put(&record, Precondition::MustNotExist)`; `AlreadyExists` counts as success. A store error is logged (no values) and does not fail the swipe: the user's action matters more than the experiment `[DEFAULT]`.
8. Return `Some(eval_id)`; put it in the undo token.

`mark_undone` (API-SW-2): if the undo token has an `eval_id`, read the record, set `outcome.undone = true`, write with `Precondition::Matches(version)`; missing record (TTL, opt-out) is fine. Undoing an undo is not possible (the app's stack pops), so the flag only goes one way.

The label itself is never stored; T-907's `label_for(direction, undone)` derives it in the report.

## Acceptance criteria

| ID | Behaviour (one line) |
| --- | --- |
| CL-03 AC1 | A consenting user's swipe records both models' predictions for the card |
| CL-03 AC2 | A model failure is recorded on the record for that model only |
| CL-03 AC3 | One record per swipe with both predictions, header rules, action, undo status and time to swipe; no sender, subject, text or message ID |
| BAKE-6 | One swipe writes one record carrying both predictions; undo flips the same record; down writes nothing |
| EXP-1 | Records hold only allowed fields, no corpus string, a 180-day expiry, and no record for unswiped cards or after opt-out |

## Tests that must pass

- `cl_03_ac1_swipe_records_both_predictions` (service integration with `FakeClassifier`s)
- `cl_03_ac2_failed_model_recorded_with_error_code` (service integration: Jev times out; record has `jev.error_code = timeout` and a full Gemini prediction)
- `cl_03_ac3_one_record_per_swipe` (service integration)
- `cl_03_ac3_retried_swipe_writes_one_record` (service integration: same `Idempotency-Key` twice)
- `cl_03_ac3_record_has_no_sender_subject_text_or_message_id` (service integration)
- `bake_6_swipe_labels_both` (service integration: left, right and up each give one record with the right direction)
- `bake_6_undo_flips_same_record` (service integration)
- `bake_6_down_writes_no_record` (service integration)
- `exp_1_eval_record_holds_no_corpus_string` (service integration over the corpus: serialise each record and search for every canary, corpus address, domain, subject fragment, URL and message ID)
- `exp_1_eval_record_fields_match_allowed_schema` (service integration: the JSON key set equals the S5 allowed set exactly, recursively)
- `exp_1_eval_id_not_from_message_id` (unit: same user and key with two different message IDs give the same eval ID; different keys differ)
- `exp_1_unswiped_card_leaves_no_record` (service integration)
- `exp_1_record_has_180_day_expiry` (unit)
- `exp_1_no_record_after_opt_out` (service integration: opt out between Feed and swipe)
- `eval_id_differs_from_job_id` (unit: same inputs as the T-605 job ID derivation give a different UUID)

## Edge cases and traps

- Never put the message ID, mailbox ID, sender, subject or any text in the record; the payload has them for other purposes, so build the record field by field, never by copying a struct wholesale.
- `time_to_swipe_ms` comes from `Clock` at swipe time minus `issued_at` in the token; never from the client.
- A failed eval write must not fail the swipe or its undo.
- `SwipeDirection` lives in `ports` (T-201b); if T-907 defines its own in `domain`, convert explicitly.
- Do not write a record for a non-consenting user even if a stale token holds predictions.

## Out of scope

- Kill switches and ADM-8, ADM-9: T-906b. The report: T-908a, T-908b. Deleting records on opt-out: T-902; on TTL: T-706.

## Done when

- The tests above pass and every required check is green (S10 10.1).
- Definition of done in S10 10.4.
