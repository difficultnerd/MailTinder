# T-901: Classifier trait wiring and sealed classification

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M9 | sonnet | about 450 lines of code plus tests | T-103, T-602c, T-604 |

**Read only these spec sections:** S2 CL-01 AC1, AC3 and CL-03 AC1, AC2, AC4 (`docs/specs/S2-v1-acceptance-criteria.md`), S4 5.1, 5.2, 5.3, 5.4 and the S4 5.7 bullet "Model calls run in parallel" (`docs/specs/S4-architecture.md`), S7 5.4 `Card` rows `classification_token` and `classifier_id`, S7 5.5 the `classification_token` bullet, S7 5.13 "How cards carry the bake-off" (`docs/specs/S7-api-contract.md`), S10 9.1 `FakeClassifier` and the scenario table, S10 9.2 rows BAKE-2 to BAKE-5 (`docs/specs/S10-test-strategy.md`). Nothing else is needed.

## Goal

Every card's classification goes through one pipeline: header rules, then the header guard, then (for a consenting user with a model switched on) Gemini and Jev in parallel behind the `Classifier` trait, with a 2-second timeout and a concurrency cap. The badge stays on header rules. Every prediction and failure is sealed into the card's `classification_token`, so the swipe can record it later without calling a model again. After this task, T-904 and T-905 only need to implement the trait.

## Files

| Action | Path | What |
| --- | --- | --- |
| Create | `backend/crates/api/src/classify/mod.rs` | `ClassifierSet`, `BakeoffGate`, `classify_page` |
| Create | `backend/crates/api/src/classify/payload.rs` | `BakeoffPayload`, conversions to `ModelPrediction` |
| Create | `backend/crates/api/src/classify/input.rs` | `build_input` (stub until T-903) |
| Change | `backend/crates/api/src/routes/feed.rs` | Extend T-602c's `ClassificationPayload`; call `classify_page` in step 11 |
| Change | `backend/crates/api/src/services/swipe.rs` (T-604's path) | Open the extended token; re-apply the guard; no model call |
| Create | `backend/crates/testkit/src/fake_classifier.rs` | `FakeClassifier` |
| Create | `backend/crates/testkit/src/contract/classifier.rs` | `classifier_contract(make: impl Fn() -> Arc<dyn Classifier>)` |
| Create | `backend/crates/api/tests/classify.rs` | Service integration and property tests |

## Types and signatures

```rust
// api/src/classify/mod.rs
pub const MODEL_TIMEOUT: Duration = Duration::from_secs(2);   // S4 5.7, CR-01 T-new-3 [TUNABLE]
pub const MODEL_CONCURRENCY: usize = 8;                       // S4 5.7 per model per Feed request [TUNABLE]
pub const PRICE_VERSION: &str = "1";                          // T-908a's price table key [DEFAULT]

pub struct ClassifierSet {
    pub gemini: Option<Arc<dyn Classifier>>,                  // None until T-904 is wired
    pub jev: Option<Arc<dyn Classifier>>,                     // None until T-905 is wired
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct BakeoffGate { pub gemini: bool, pub jev: bool }    // computed per request: consent (T-902) AND switch (T-906b)

pub struct CardToClassify<'a> { pub meta: &'a MessageMeta, pub stripped_text: &'a str }
pub struct ClassifiedCard { pub badge: Classification, pub payload: ClassificationPayload }

/// Header rules and guard for every card; then, if the gate is open for a model, that model for every
/// card in parallel. Never returns an error because of a model.
pub async fn classify_page(set: &ClassifierSet, gate: BakeoffGate, now: OffsetDateTime,
                           cards: &[CardToClassify<'_>]) -> Vec<ClassifiedCard>;

// api/src/routes/feed.rs (T-602c type, extended; serde field names are part of the sealed format)
#[derive(Serialize, Deserialize)]
pub struct ClassificationPayload {
    pub mailbox_id: Uuid, pub message_id: String,
    pub header_rules: Classification, pub classifier_id: String,   // "header_rules@1"
    pub issued_at: OffsetDateTime,                                  // for time to swipe (T-906a)
    pub bakeoff: Option<BakeoffPayload>,                            // None when the gate was closed for both models
}

// api/src/classify/payload.rs
#[derive(Serialize, Deserialize)]
pub struct BakeoffPayload {
    pub gemini: Option<ModelPrediction>,   // None: not called (switched off). Some with error_code: called and failed
    pub jev: Option<ModelPrediction>,
    pub header_facts: EvalHeaderFacts,     // T-201b
    pub provider: Provider,
    pub age_bucket: AgeBucket,             // T-201b
    pub text_tokens_bucket: TextTokensBucket,
    pub lang_is_english: bool,
    pub input_version: String, pub question_version: String, pub price_version: String,
}
pub fn age_bucket(received_at: OffsetDateTime, now: OffsetDateTime) -> AgeBucket; // <7d, 7d-90d, 90d-1y (365 d), 1y-5y, >5y; lower bound inclusive
pub fn eval_header_facts(facts: &HeaderFacts) -> EvalHeaderFacts;
pub fn prediction_ok(id: &ClassifierId, c: &Classification, latency_ms: u32, input_tokens: Option<u32>) -> ModelPrediction;
pub fn prediction_err(id: &ClassifierId, e: &ClassifierError, latency_ms: u32) -> ModelPrediction;

// api/src/classify/input.rs
/// Until T-903 lands: subject and text are empty strings, input_version "0". T-903 replaces the body.
pub fn build_input(meta: &MessageMeta, stripped_text: &str) -> ClassifierInput;

// testkit/src/fake_classifier.rs
pub enum Scripted { Answer(Classification), Error(ClassifierError), Delay(Duration, Box<Scripted>) }
pub struct FakeClassifier { /* id, script per message (by subject canary or by call order), call log */ }
impl FakeClassifier {
    pub fn new(id: &str) -> Self;
    pub fn script_all(&self, s: Scripted);
    pub fn calls(&self) -> Vec<ClassifierInput>;   // what it received, for byte-identity checks
    pub fn max_in_flight(&self) -> usize;
}
```

`ModelPrediction`, `ModelErrorCode`, `EvalHeaderFacts`, `AgeBucket`, `TextTokensBucket` are T-201b record types. `ClassifierInput`, `ClassifierError`, `Classifier`, `ClassifierId` are T-201a. `HeaderRules::classify` and `header_guard` are T-102 and T-103.

## Algorithm

`classify_page`:

1. For each card: `hr = HeaderRules::classify(&meta.facts, &meta.sender)`; `badge = header_guard(&meta.facts, &hr, Some(&hr)).classification` (CL-01 AC3: during the bake-off the candidate is header rules itself).
2. If `!gate.gemini && !gate.jev`: payload with `bakeoff: None`; done.
3. Build one `ClassifierInput` per card with `build_input` and keep it in an `Arc`, so both models get the same value (BAKE-2 byte-identical input).
4. For each open model, run all its calls concurrently with `futures::stream::iter(..).map(..).buffer_unordered(MODEL_CONCURRENCY)` (or a `tokio::sync::Semaphore` with 8 permits). Gemini and Jev streams run at the same time (`tokio::join!`), so neither waits for the other.
5. Each call: `start = tokio::time::Instant::now()`; `tokio::time::timeout(MODEL_TIMEOUT, model.classify(&input))`. Outcomes:
   - `Ok(Ok(c))`: validate `c.bulk_score <= 100`, `confidence` finite in 0 to 1, each probability finite in 0 to 1; pass gives `prediction_ok`, fail gives `prediction_err(InvalidOutput)`.
   - `Ok(Err(e))`: `prediction_err(e)`.
   - `Err(_elapsed)`: `prediction_err(ClassifierError::Timeout)`.
   - `latency_ms` = elapsed milliseconds, saturating at `u32::MAX`.
6. `ModelPrediction.model_version` = the model's `id().0` (for example `jev@1.13.0`). `input_tokens` = T-903's `approx_tokens(render_model_text(&input))`, the same for both models (`None` until T-903 lands). Error mapping is one to one: `Timeout`, `Http(_)` to `Http`, `InvalidOutput`, `Disabled`, `Unavailable`.
7. The model's answer never reaches `header_guard` for the badge and never reaches the card DTO (BAKE-3). The badge is always step 1's. (Recording guard disagreements is for the day a model is promoted; not needed now.)
8. Build `BakeoffPayload` with buckets from `build_input`'s side data (T-903 adds `text_tokens_bucket` and `lang_is_english`; until then `Under100` and `false`), `provider = meta.mailbox`'s provider, versions from the input (`input_version`, T-903's `QUESTION_VERSION` or "0", `PRICE_VERSION`).
9. The Feed waits for the model calls of its page, which are bounded by `MODEL_TIMEOUT`; it never fails because of a model `[DEFAULT: see trap 1]`.

Feed (T-602c step 11): replace the direct header-rules call with `classify_page`; seal the extended `ClassificationPayload` with `issued_at = now`. `classifier_id` on the card stays `"header_rules@1"` for admins only.

Swipe (T-604):

1. Open the token as `TokenType::Classification`. A well-formed token of another type, or a payload naming another mailbox or message: `400 invalid_request`. Any other open failure (an earlier session, expiry or tampering) is not an error: the swipe proceeds and no `classifier_eval` record is written (S7 5.5; see trap 2).
2. Re-read the message from the provider (already done by T-604), run `HeaderRules::classify` and `header_guard` on the fresh headers; that result, not the token, decides the action.
3. Make no model call. Keep the opened payload for T-906a.

## Acceptance criteria

| ID | Behaviour (one line) |
| --- | --- |
| CL-01 AC1 | Every classification goes through the classifier pipeline and the guard; a new implementation needs no change outside the classifier module |
| CL-01 AC3 | During the bake-off the card badge comes from header rules only |
| CL-03 AC1 | With the gate open, Gemini and Jev both classify each card in parallel with identical input |
| CL-03 AC2 | A model error, 2-second timeout or invalid output is recorded for that model; the card is unaffected |
| CL-03 AC4 | The swipe accepts only a valid sealed token for that message and re-applies the guard |
| BAKE-2 | One request per model per card, in parallel, identical input, concurrency within the cap |
| BAKE-3 | Card class, badge and reason equal header rules whatever the models say |
| BAKE-4 | Each failure scenario leaves the card unchanged and records the right error code |
| BAKE-5 | The sealed token carries both predictions; a token that opens but names another message is refused, while an unopenable token (expiry, earlier session, tampering) lets the swipe proceed with no eval record; no model call at swipe |

## Tests that must pass

- `cl_01_ac1_classification_goes_through_pipeline` (service integration: a `FakeClassifier` registered in `ClassifierSet` is called with no Feed code change)
- `cl_01_ac3_badge_from_header_rules_only` (service integration)
- `cl_03_ac1_both_models_called_in_parallel` (service integration with paused tokio time: each fake delays 1 s; the page completes at 1 s, not 2 s)
- `cl_03_ac2_timeout_recorded_card_unaffected` (service integration: fake delays 10 s; Feed completes at 2 s virtual time; `error_code = timeout`)
- `cl_03_ac4_open_failure_swipe_proceeds` (service integration: a tampered token, or one sealed to an earlier session, lets the swipe proceed with no eval record)
- `cl_03_ac4_swipe_reapplies_guard` (service integration: token says `list`, fresh headers say `bulk_no_header`; no job queued)
- `bake_2_both_models_called_per_card` (service integration: 20 cards, 20 calls each, the two `ClassifierInput` logs equal element by element)
- `bake_2_concurrency_never_exceeds_cap` (service integration: 50 cards, `max_in_flight() <= 8` per model)
- `bake_3_badge_stays_on_header_rules` (property: any scripted model output; card DTO fields equal the header-rules result; the JSON response holds no model field)
- `bake_4_failure_isolated` (service integration, table: `Timeout`, `Http(429)`, `Http(500)`, `Http(503)`, `Unavailable`, `InvalidOutput`, out-of-range score, NaN confidence; each for Gemini alone, Jev alone and both; card unchanged, right `error_code`)
- `bake_5_sealed_token_carries_both` (service integration: open the token in the test and find both predictions)
- `bake_5_no_model_call_at_swipe` (service integration: fake call counts unchanged by the swipe)
- `bake_5_expired_token_swipe_proceeds` (service integration, clock past 12 hours)
- `classifier_contract_header_rules_and_fake` (contract: `testkit::contract::classifier` passes for a `HeaderRules` adapter and `FakeClassifier`)
- `age_bucket_boundaries` (unit: 6 d 23 h, 7 d, 90 d, 365 d, 5 y)

## Edge cases and traps

1. S10 9.1 says a slow model must not delay the Feed page, yet the predictions must be inside the token the page returns. Read it as "not beyond the 2-second timeout": the page waits at most `MODEL_TIMEOUT` for models. Prefetching in the app hides it (CR-01 1.7).
2. S7 5.5 says an unopenable token "from an earlier session is not an error", and AES-GCM cannot tell an earlier session from a tampered token (S6 5), so all unopenable cases behave the same: only a well-formed token of the wrong type, or one that opens but names another mailbox or message, is refused with `400`; every other open failure lets the swipe proceed and writes no `classifier_eval` record (S7 5.5, T-604). The action comes from header rules re-run on freshly read headers, so an unopenable token cannot drive it.
3. Use `tokio::time` (pausable) for timeouts and latency, and `Clock` for `issued_at`. Never `std::time::Instant` or `SystemTime`.
4. Do not call a model when its gate flag is false, not even to warm up.
5. Model output must never appear in the card DTO, logs or errors, even for admins.
6. `ClassifierInput` holds message text; never `Debug` it (T-201a writes `Debug` as `..`), never log it.
7. Until T-903 is merged, `build_input` must send no subject or text. The gate stays closed in production until T-906b anyway (kill switches default off), but do not rely on that alone.
8. HeaderRules is a static function (T-102), not a `Classifier` object; wrap it in a small `HeaderRulesClassifier` adapter only for the contract suite.

## Out of scope

- Consent and the gate's inputs: T-902 and T-906b. Real models: T-904, T-905. Redaction: T-903.
- Writing the `classifier_eval` record: T-906a.

## Done when

- The tests above pass and every required check is green (S10 10.1).
- Definition of done in S10 10.4.
