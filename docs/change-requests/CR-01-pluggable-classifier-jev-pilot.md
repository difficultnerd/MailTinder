# CR-01: Pluggable classifier, Gemini versus Jev bake-off

Status: change request for the product plan thread (owner of CONTEXT.md, roadmap and specs). Raised 3 October 2026.
Requested by James, 3 October 2026: "I would like the application architecture built such that I could integrate Jev as my classifier in v1. I want to experiment with it. We can A/B test during our pilot phase before Google's security requirements become problematic, we can collect data and publish independent results."
Clarified by James, 3 October 2026: "I think it would be interesting to pass email to both classifiers, Google's and Jev, and see which one is more accurate based on user feedback. Given the amount of email I have to sort, it would be interesting enough to get a feel quickly."
Background: `research/jev-classifier-evaluation.md`.

## Summary

Put the message classifier behind a trait, the same way the provider adapters are. Ship three implementations in v1: header rules (default, always runs), Gemini on Vertex AI and Jev. For consenting users, every card goes to both Gemini and Jev with identical input, and the user's swipe scores both. This is a head-to-head bake-off, not a split of users: James's own backlog alone gives thousands of labelled comparisons quickly. Header facts keep a hard veto over anything destructive.

This reverses one recommendation in D9 ("no server-side LLM in v1") for pilot users who opt in. Jev is not an LLM, but it is a server-side third-party model that receives mail content, which is what D9 was guarding against.

## 1. Design

### 1.1 Classifier trait (S3, S4)

```
trait Classifier {
    fn id(&self) -> ClassifierId;            // "header_rules@1", "jev@1.13.0"
    async fn classify(&self, input: &ClassifierInput) -> Result<Classification, ClassifierError>;
}

Classification { class, bulk_score, bulk_reason, confidence, probabilities }
```

- `HeaderRules` implements it today with no change in behaviour.
- `JevClassifier` calls `POST https://api.typesafe.ai/v1/systemone` with the model pinned to `jev-1.13.0` (never `jev-latest`, whose answers can change without notice). One request carries all questions:
  - `class`: Choice over the five S3 classes, options in a fixed order.
  - `bulk`: Score on a 0 to 100 rubric.
  - Room for a filing Choice later; not in this CR.
- `GeminiClassifier` calls Gemini on Vertex AI in `us-central1` (nearest Gmail data; James, 3 October 2026) (default model: Gemini Flash-Lite, the cheapest current tier), with the same input, the same five-class enum through `responseSchema`, and token log probabilities as its confidence. Same question wording as Jev so the comparison is fair.
- Gemini Nano in Chrome stays the v1 on-device plan for users who do not opt in. It is left out of the bake-off because its availability and model version vary by device.

### 1.2 Header facts keep the veto (S3)

Once a model drives the badge and class, a guard runs after any classifier and clamps the result to what the headers prove:

| Header fact | Guard |
| --- | --- |
| No valid `List-Unsubscribe` (or DKIM does not cover it) | Class cannot be `list`; no unsubscribe job is ever created. Matches James's rule: never unsubscribe without the header. |
| Header rules say `personal` with high confidence | Jev may not move it to `list` or `suspect` on its own; the card shows the header-rules class and the disagreement is recorded. |
| Model error, timeout or invalid output | Record the failure for that model; the card is unaffected because it shows header rules. |

In the bake-off neither model drives an action, so it is safe on real inboxes. The guard matters once a winning model is promoted to the badge.

### 1.3 Sealed classification on the card (S7)

S7 says the server re-classifies on swipe and never trusts a class from the client. With a remote classifier, re-calling Jev at swipe time costs a round trip and can disagree with what the user saw. Instead, `feed/next` returns a server-sealed classification token (the one sealing scheme in S6 section 5, under the user's `data_key`: AEAD over message ID, mailbox ID, the badge class, each model's prediction, classifier IDs, expiry). The swipe handler verifies it, re-reads the message headers from the provider and re-applies the header guard. The class is still server-produced, so the S7 principle holds.

### 1.4 Bake-off

- For every consenting user, Gemini and Jev both classify every card, in parallel. No user split.
- **The card shows the header-rules badge, not either model's answer.** If the badge came from Jev or Gemini, the user's swipe would partly follow the badge and the feedback would favour whichever model was shown. Keeping the badge neutral makes the swipe an unbiased label for both.
- Optional later phase: once one model leads clearly, show its badge to measure whether it speeds up sorting (swipe time, undo rate). Not needed to answer "which is more accurate".
- Users who have not consented never reach Gemini or Jev.

### 1.5 What goes to Gemini and Jev (S5)

Built in memory, never stored:

- Header allowlist: `From` display name and domain, `List-Id`, `List-Unsubscribe` presence (not the URL), `List-Unsubscribe-Post`, `Precedence`, `Auto-Submitted`, ESP fingerprint header names, a summarised `Authentication-Results` (pass or fail per mechanism).
- Subject.
- Stripped plain text, truncated to about 500 tokens. URLs, email addresses and long digit runs replaced with placeholders before sending.
- Never: `To`, `Cc`, recipient addresses, attachments, message IDs, the unsubscribe URL.

About 700 to 1,000 tokens a message; at the pilot's volume that is a few US dollars a month.

### 1.6 Evaluation record (S5)

New Firestore collection `classifier_eval`, class C1, TTL 180 days `[TUNABLE]`:

| Field | Notes |
| --- | --- |
| `eval_id` | UUID v5 of the user ID and the swipe's `Idempotency-Key` (S7 definition, adopted 3 October 2026, spec audit), so a retried swipe cannot double-count; not derived from the message ID |
| `user_pseudo_id` | HMAC of user ID, as in logs |
| `header_rules` | class, score |
| `gemini` | class, score, log-probability confidence, model version, latency ms, input tokens, error code |
| `jev` | class, score, probabilities, confidence, model version, latency ms, input tokens, error code |
| `outcome` | swipe direction, undone (yes or no), time to swipe ms |
| `header_facts` | booleans only (has List-Unsubscribe, DKIM covers it, has List-Id) |

No sender, subject, text or message ID. The record is written when the swipe lands (the sealed token carries the predictions forward), so cards never swiped leave nothing behind.

Ground truth comes from the swipes. Mapping: left swipe with no undo means junk to the user (`list`, `bulk_no_header` or `suspect`); right or up means wanted; undo flips the label; down (skip) is not a label. Swipes measure "does the user want this", which is close to but not the same as the five classes, so the report shows both agreement with swipes and per-class confusion. James's labelled E2 set gives the class-level benchmark.

Report (admin only, computed from `classifier_eval`): accuracy against swipes per model, calibration (reliability curve), agreement rate between the models, latency and cost per 1,000 messages, and a running tally so James can watch it converge.

### 1.7 Operations (S4)

- Egress is enforced per service at the `HttpEgress` port; `api`'s allowlist includes `api.typesafe.ai` and Vertex AI alongside the mail, storage and OAuth hosts (S4 5.7, 3 October 2026, spec audit).
- Jev API key in Secret Manager; `api` is the only accessor of that secret; rotated quarterly. Vertex uses the `api` service account with `roles/aiplatform.user`; no key.
- Vertex: turn off prompt caching for the project and request the abuse-monitoring logging exception for zero data retention.
- Kill switches: environment variables `CLASSIFIER_JEV_ENABLED` and `CLASSIFIER_GEMINI_ENABLED`, read at start-up plus a Firestore config doc checked every minute, so the admin can turn Jev off without a deploy.
- Feed calls Jev for a page of cards concurrently (capped at 8 in flight per request), within TypeSafe's 80 requests a second limit. Prefetching hides the latency.
- Billing alert on the TypeSafe account.

## 2. Consent and policy (S2, S9, S5)

- **Consent screen** (Settings, Experiments): "Try an experimental classifier. Card text (sender, subject and the first part of the message) is sent to Google (Vertex AI, United States) and TypeSafe AI (United States) to compare two classifiers. Neither trains on it. TypeSafe has not yet committed to how long it keeps this data and has no data processing agreement or security attestation in place. Anonymous accuracy figures from your swipes may be published. Anonymous totals already published or saved stay as they are if you later opt out." (Last sentence added by CR-01a; TypeSafe sentence added 3 October 2026, spec audit Q4: consenting friends' mail may go to Jev before a DPA, and Jev stays open to all consenting users.) Off by default. Turning it off stops all calls immediately and deletes that user's `classifier_eval` records.
- **Google's rules apply now.** CASA is deferred until we leave Testing mode, but the Limited Use policy applies from the first user. This design stays inside it: the transfer serves a user-facing feature (the card badge), happens only with consent, and the receiving model does not train on it. Our reading of "generalised AI model" transfers is that a model that does not train on inputs is not covered; Google's reviewers make the final call at verification.
- **Publishing results.** Most of the data will come from James's own mailboxes, which raise no third-party question. For other pilot users, Limited Use restricts use of Gmail data to providing and improving user-facing features. Aggregate accuracy figures derived from pilot users' swipes sit at the edge of that. Two safeguards: the consent text names publication explicitly, and the headline benchmark uses James's own labelled mail from spike E2, which carries no third-party restriction. Publish aggregate metrics only (accuracy, calibration, latency, cost), never examples from users' mail.
- **Before verification:** either TypeSafe supplies a DPA, a retention commitment and a security attestation, or Jev is switched off and its code path removed from the verified build.

## 3. Security (S6)

New threat rows:

| ID | Threat | STRIDE | Mitigation | ASVS |
| --- | --- | --- | --- | --- |
| T-new-1 | Marketing email crafted to make a model mark it personal or transactional | Tampering | Header guard (1.2); Jev cannot create unsubscribe or spam actions on its own; fixed question text; email content is only ever `state` | V15 (secure coding and architecture) |
| T-new-2 | Mail content retained or exposed by TypeSafe or Vertex | Information disclosure | Allowlisted, redacted input (1.5); consent; pilot only; kill switch; vendor terms gate before verification | V14 (data protection) |
| T-new-3 | Model outage or slow responses stall the Feed | Denial of service | Both calls in parallel, off the card's critical path (the badge is header rules); 2 s timeout, fallback to header rules, concurrency cap | V15 |
| T-new-4 | Jev API key leaked | Information disclosure | Secret Manager, single accessor, never logged, rotation | V13 (configuration) |

Response validation: parse into typed Rust structs with `deny_unknown_fields`; class must be one of the five enums; probabilities finite and in 0 to 1.

## 4. Edits by document

| Doc | Edit |
| --- | --- |
| CONTEXT.md | Note the pilot classifier experiment and its consent gate |
| Roadmap | D9 amended: no server-side model in v1 except the opt-in Gemini versus Jev bake-off. New experiment E4: the bake-off |
| S2 | New story and ACs for the Experiments setting (opt in, opt out deletes eval records, no model call without consent) and the admin bake-off report |
| S3 | Classifier trait, `ClassifierId`, header guard rules, swipe-to-label mapping, `ClassifierEval` entity |
| S4 | `GeminiClassifier` and `JevClassifier` in the component view; Vertex AI and TypeSafe rows in the Services table; egress, secret, kill switches |
| S5 | `classifier_eval` collection; `users.experiments_consent`; Vertex AI and TypeSafe AI in "Third parties that receive data"; amend "No AI vendors in v1" |
| S6 | Threat rows above; Jev key in the cryptographic inventory |
| S7 | Sealed classification token on cards (carries both predictions to the swipe); `PUT /me/experiments`; admin `GET /admin/bakeoff` report |
| S9 | Experiments section in Settings with the consent text |
| S10 | Tests: no Gemini or Jev call without consent (mock server asserts zero requests), header guard property tests, fallback on timeout, input redaction test against the fixture corpus, eval record contains no fixture string |

## 5. Defaults chosen (James can override)

1. "Google's classifier" means Gemini Flash-Lite on Vertex AI in `us-central1`, given identical input. Gemini Nano is left out because it varies by device.
2. The card badge stays on header rules during the bake-off so swipes are unbiased.
