# CR-01a: Bake-off statistics fit for publication

Status: approved by James, 3 October 2026. Change request delta on CR-01 for the product plan thread to apply.
Requested by James, 3 October 2026: "On the classifier bake-off, I would like to be collecting data sufficient to write a reasonable blog entry where I publish my findings on which classifier method is better or worse. I think that would be interesting data for the community to know. Do we have functionality to support collecting those statistics?"
Checked against: S4 section 5.6, S5 `classifier_eval`, S7 section 5.13 (ADM-10), S10 section 9.4, as of 3 October 2026.

## What already works

`classifier_eval` already records each model's class, score, confidence, latency and tokens next to the swipe outcome. ADM-10 already reports accuracy against swipes, a class-by-label confusion table, calibration bins with ECE, agreement between the models, latency p50 and p95, error and timeout rates, cost per 1,000 messages and a daily trend, all with small-cell suppression. That is enough for a private "which looks better" read. These gaps stop it being a result someone else could trust.

## Gaps and changes

All changes stay content-free (C1 fields, booleans and buckets only) and aggregate-only on the way out.

### G1. Header rules missing as a baseline in the report (S7 ADM-10, S10 9.4)

`classifier_eval` stores the header-rules class and score, but ADM-10's `models` array and `trend` only describe Gemini and Jev. A write-up needs the free baseline to show what the paid models add.

- ADM-10 `models` includes `header_rules@1` with the same fields as the models (cost 0, latency measured in process). `bulk_score / 100` is its confidence for the calibration curve.
- `trend` and every new section below include all three.

### G2. No binary precision and recall (S7 ADM-10)

Accuracy hides the error that matters. Calling a wanted email junk costs the user more than the reverse.

- Per model, on the junk versus wanted labels: precision, recall and F1 for `junk`, plus the false-junk rate (wanted mail predicted junk).

### G3. No uncertainty or paired comparison (S7 ADM-10, S10 9.4)

Today the report gives point estimates only. Most labels will come from one person, so swipes are not independent, and the two models answer the same cards, which allows a much stronger comparison than two separate accuracies.

- Wilson 95% interval on every rate.
- Paired comparison on cards where all compared models answered: counts of cards where only Gemini was right, only Jev was right, both, neither; McNemar's exact test p value; accuracy difference with a 95% bootstrap interval resampled by participant (cluster bootstrap), so heavy users cannot overstate certainty.
- `n_paired` (cards where both models returned a valid answer) beside `cards`, so availability failures do not quietly shrink the comparison.
- Participant contribution: number of participants and the share of labels from the largest contributor (a single percentage, suppressed below `min_cell_size`). An honest write-up has to say "mostly one inbox".

### G4. No breakdown by mail type (S4 5.6, S5, S7 ADM-10)

Without ground-truth mail types, segments come from facts we already hold or can add as coarse buckets:

| New `classifier_eval` field | Values | Why |
| --- | --- | --- |
| `provider` | `gmail`, `graph` | Gmail versus Outlook mail |
| `age_bucket` | `<7d`, `7d-90d`, `90d-1y`, `1y-5y`, `>5y` | Backlog versus new mail; models may degrade on old formats |
| `text_tokens_bucket` | `<100`, `100-300`, `>300` | Jev's documented weakness with irrelevant context |
| `lang_is_english` | boolean from a local detector | Non-English is a known weak spot for Jev |

ADM-10 adds `by_segment`: accuracy, false-junk rate and `n` for each model, segmented by `header_rules` class, each existing `header_facts` boolean and each new field above. Small-cell suppression applies per segment.

### G5. Method versions not recorded (S4 5.6, S5)

If the question wording, the input template or a model version changes mid-run, results from before and after get mixed.

- New fields: `input_version` (redaction and truncation rules), `question_version` (prompt and question wording), `price_version` (price table used for cost). Model versions are already stored.
- ADM-10 filters by these and refuses to pool across different `question_version` or `input_version` values unless asked.

### G6. 180-day TTL loses the evidence (S4 5.6, S5, S7)

The report is computed on demand from raw records. After 180 days the early data is gone, and any opt-out deletes rows, so the numbers in a published post could not be reproduced or extended later.

- New ADM-11 `POST /admin/bakeoff/snapshots` with the ADM-10 query: stores the computed aggregate (after small-cell suppression) in a new collection `bakeoff_snapshots` (C1, aggregate only, no pseudonymous IDs, kept until the admin deletes it). ADM-12 `GET /admin/bakeoff/snapshots/{id}` returns it.
- Raw `classifier_eval` keeps its 180-day TTL and the opt-out deletion. Snapshots are not recomputed after an opt-out.
- Consent text gains one sentence: "Anonymous totals already published or saved stay as they are if you later opt out."

### G7. Export for charts (S7 ADM-10)

- Latency as a histogram (fixed bins), not only p50 and p95.
- `trend` carries per-model Wilson intervals and `n` per day.
- ADM-10 and ADM-12 accept `Accept: text/csv` and return one tidy table (`section, model, segment, metric, value, lower, upper, n`) so charts can be built without client conversion. Same suppression rules.

### G8. Swipes are a noisy label (methodology, S2 or E4)

A right swipe can mean "I like this newsletter". The E2 set (James's own mail, labelled by him, run locally through `tools/`) already gives the class-level benchmark and only commits aggregates. Add: E4 reports how far swipe labels agree with E2 labels on the overlapping messages, so the post can state how noisy swipe ground truth is. Compute locally; commit the agreement figure only.

## Limited Use and data minimisation

- Every new field is a bucket, boolean or version string; none comes from mail content except `age_bucket` (from the message date) and `lang_is_english` (computed in memory, text never stored).
- Outputs stay aggregate with small-cell suppression; snapshots store nothing per user.
- Publication is already named in the consent text; G6 adds the opt-out sentence.

## Edits by document

| Doc | Edit |
| --- | --- |
| S2 | CL stories: report includes header rules, intervals, paired test, segments; snapshot and CSV export; consent sentence |
| S3 | `ClassifierEval` gains the G4 and G5 fields; new `BakeoffSnapshot` entity |
| S4 5.6 | Same fields; snapshot collection; report contents per G1 to G7 |
| S5 | `classifier_eval` row updated; new `bakeoff_snapshots` row (C1, admin-deleted); EXP-1 schema check updated |
| S7 | ADM-10 response extended (G1 to G4, G7); ADM-11 and ADM-12; rate limits; consent text |
| S9 | Admin bake-off panel: snapshot button, CSV download |
| S10 | 9.4 tests: Wilson and McNemar against known values, cluster bootstrap with a fixed seed, segment suppression, no pooling across versions, snapshot contains no pseudonymous ID, CSV matches JSON |
