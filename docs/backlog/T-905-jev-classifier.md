# T-905: Jev classifier

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M9 | sonnet | about 300 lines of code plus about 300 lines of tests (including `fake-jev`) | T-901, T-903 |

**Read only these spec sections:** S4 5.1 table row `JevClassifier` and the paragraph under the table, S4 5.7 bullets on the Jev key, timeouts and the 8-in-flight cap (`docs/specs/S4-architecture.md`), CR-01 1.1 bullet `JevClassifier` and CR-01 3 "Response validation" (`docs/change-requests/CR-01-pluggable-classifier-jev-pilot.md`), S10 9.1 `fake-jev` bullet and the scenario table (`docs/specs/S10-test-strategy.md`), S2 CL-03 AC2 (`docs/specs/S2-v1-acceptance-criteria.md`), `research/jev-classifier-evaluation.md` section "Option" table and the three question types. Nothing else is needed.

## Goal

`JevClassifier` implements the `Classifier` trait by calling TypeSafe's Jev API (`POST https://api.typesafe.ai/v1/systemone`, model pinned to `jev-1.13.0`) with one request holding two questions: a Choice over the five classes in fixed order and a Score for bulk. It validates the answer strictly and returns the class, score, the full class probability distribution and a confidence. `fake-jev` stands in for the API in tests.

## Files

| Action | Path | What |
| --- | --- | --- |
| Create | `backend/crates/adapters-models/src/jev.rs` | `JevClassifier`, wire structs |
| Change | `backend/crates/adapters-models/src/lib.rs` | `pub mod jev;` |
| Create | `backend/crates/testkit/src/fake_jev.rs` | `fake-jev` axum server with scenario switches |
| Change | `backend/crates/api/src/main.rs` | Build `JevClassifier` into `ClassifierSet.jev`, key from Secret Manager |
| Create | `backend/crates/adapters-models/tests/jev.rs` | Contract suite and scenario tests |

## Types and signatures

```rust
// adapters-models/src/jev.rs
pub const JEV_MODEL: &str = "jev-1.13.0";            // pinned, never "jev-latest" (CR-01 1.1)
pub const PROD_BASE_URL: &str = "https://api.typesafe.ai";
pub const MAX_RESPONSE_BYTES: usize = 64 * 1024;
pub const PROBABILITY_SUM_TOLERANCE: f64 = 1e-3;     // [DEFAULT]

pub struct JevConfig { pub base_url: Url }           // overridable only under cfg(test) or the "testkit" feature
pub struct JevClassifier { cfg: JevConfig, egress: Arc<dyn HttpEgress>, api_key: Sensitive<String> }
impl JevClassifier { pub fn new(cfg: JevConfig, egress: Arc<dyn HttpEgress>, api_key: Sensitive<String>) -> Self; }

#[async_trait]
impl Classifier for JevClassifier {
    fn id(&self) -> ClassifierId;   // "jev@1.13.0"
    async fn classify(&self, input: &ClassifierInput) -> Result<Classification, ClassifierError>;
}

// Wire format [ASSUMES]: shape below is our best reading of the vendor's description (named questions,
// Choice returns the pick plus the full distribution, Score rates against a rubric). Confirm against
// https://docs.typesafe.ai before merging; change these structs and fake-jev together and record the
// confirmed shape in the PR.
#[derive(Serialize)] struct JevRequest<'a> { model: &'a str, state: &'a str, questions: [JevQuestion<'a>; 2] }
#[derive(Serialize)] #[serde(tag = "type", rename_all = "snake_case")]
enum JevQuestion<'a> {
    Choice { name: &'a str, prompt: &'a str, options: [&'a str; 5] },
    Score { name: &'a str, prompt: &'a str, min: u8, max: u8 },
}
#[derive(Deserialize)] #[serde(deny_unknown_fields)]
struct JevResponse { model: String, answers: JevAnswers, #[serde(default)] usage: Option<JevUsage> }
#[derive(Deserialize)] #[serde(deny_unknown_fields)] struct JevAnswers { class: ChoiceAnswer, bulk: ScoreAnswer }
#[derive(Deserialize)] #[serde(deny_unknown_fields)] struct ChoiceAnswer { choice: String, probabilities: Vec<f64>, confidence: f64 }
#[derive(Deserialize)] #[serde(deny_unknown_fields)] struct ScoreAnswer { score: f64, confidence: f64 }
#[derive(Deserialize)] #[serde(deny_unknown_fields)] struct JevUsage { input_tokens: u64 }
```

## Algorithm

Request, through `HttpEgress::call` (host `api.typesafe.ai`, in the `api` allowlist), timeout 2 s, headers `Authorization: Bearer <api_key>` and `Content-Type: application/json`:

- `model`: `JEV_MODEL`.
- `state`: `render_model_text(input)` (T-903), the same bytes Gemini receives as user content.
- `class` question: `Choice`, prompt `CLASS_QUESTION`, options the five class names in `CLASS_OPTIONS` order (`list`, `bulk_no_header`, `notice`, `personal`, `suspect`).
- `bulk` question: `Score`, prompt `BULK_QUESTION`, `min` 0, `max` 100.

Response handling, first failure wins:

1. Egress `Timeout` gives `ClassifierError::Timeout`; other egress errors give `Unavailable`.
2. Non-200 status (429, 5xx, others): `Http(status)`.
3. Wrong content type, over `MAX_RESPONSE_BYTES`, invalid JSON, or any unknown or missing field: `InvalidOutput`.
4. `model != JEV_MODEL`: `InvalidOutput`.
5. `choice` not one of the five names: `InvalidOutput`.
6. `probabilities`: exactly 5 values, each finite and in 0 to 1, sum within `PROBABILITY_SUM_TOLERANCE` of 1; else `InvalidOutput`.
7. `confidence` values finite and in 0 to 1; `score` finite and in 0 to 100; else `InvalidOutput`.
8. Return `Classification { class: choice, bulk_score: score.round() as u8, bulk_reason: "jev".into(), confidence: Some(class.confidence as f32), probabilities: Some(the five as f32, in option order) }`.

The 8-in-flight cap is T-901's per-model semaphore; this classifier does no queuing of its own.

Key: read `SecretName::JevApiKey` once at start-up (T-305); `api` is the only service that loads it. A missing key at start-up leaves `ClassifierSet.jev = None` and logs `jev_key_missing` (no value).

`fake-jev` (axum, port 0): records every request; returns 401 without the expected test key; returns 400 when `model` is not `jev-1.13.0` or the options are not in the fixed order. Scenario switches (`/__fake/scenario`): valid answer; delay N ms; 429 with `Retry-After`; 500; 503; connection reset; malformed JSON; truncated body; wrong content type; oversized body; choice outside the five; unknown field; score 101; probability NaN; probability +inf; four probabilities; probabilities summing to 0.9; missing field; `model: "jev-latest"`; a `choice` field holding "ignore previous instructions".

## Acceptance criteria

| ID | Behaviour (one line) |
| --- | --- |
| CL-03 AC2 | Jev errors, 2-second timeouts and invalid output become recorded failures; the card is unaffected |
| BAKE-4 | Every S10 9.1 failure scenario for Jev maps to the right error |

## Tests that must pass

- `classifier_contract_jev_fake_jev` (contract: T-901's classifier suite against `JevClassifier` pointed at `fake-jev`)
- `cl_03_ac2_jev_timeout_recorded` (integration, paused tokio time)
- `bake_4_jev_failure_scenarios` (integration, table: every scenario switch with its expected `ClassifierError`)
- `bake_4_jev_instruction_text_is_data` (integration)
- `jev_request_pins_model_and_option_order` (integration: `fake-jev` recorded body)
- `jev_state_equals_gemini_content` (unit: for one input, the `state` string equals `render_model_text(input)`)
- `jev_key_never_logged` (integration: captured `tracing` output holds no key bytes)
- `jev_base_url_not_overridable_in_release` (unit, documented `cfg` check)

## Edge cases and traps

- The wire shape is an assumption. Do not guess further: confirm with the vendor docs, update the structs and `fake-jev` together, and keep `deny_unknown_fields` on every response struct (S4 5.1).
- Probabilities must come back in the option order we sent; if the confirmed API returns a map, reorder by name and fail on any missing name.
- `score.round()` only after the range check; never cast a NaN.
- The API key is `Sensitive`; never put it in a URL, a log line or an error.
- Never send `jev-latest`.
- TypeSafe's terms gate (no DPA yet) is a consent-text and kill-switch matter (T-902, T-906b), not code here.

## Out of scope

- Fan-out, timeout and the 8-in-flight cap: T-901. Redaction and question text: T-903. Kill switch: T-906b.

## Done when

- The tests above pass and every required check is green (S10 10.1).
- Definition of done in S10 10.4.
- The PR records the confirmed Jev request and response shape with a link to the vendor page.
