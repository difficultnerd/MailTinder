# T-102: Header facts and the HeaderRules classifier

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M1 | sonnet | about 300 lines of code plus tests | T-101 |

**Read only these spec sections:** S3 "Message classes" and "Classification" (`docs/specs/S3-domain-model.md`); S4 5.1 and 5.2 (`docs/specs/S4-architecture.md`); S2 SW-03 AC2 and AC5, UN-02 AC1, AC2, AC2a, UN-03 AC3, UN-04 AC6, CL-01 AC2 (`docs/specs/S2-v1-acceptance-criteria.md`); S10 5 "Required cases" table (`docs/specs/S10-test-strategy.md`); S6 6 first bullet. Nothing else is needed.

## Goal

A pure, deterministic `HeaderRules` classifier in `domain` turns `HeaderFacts` plus the `SenderKey` into a `Classification` (class, bulk score, plain-words reason) and picks the one unsubscribe route a reject may use. It is the badge for every card during the bake-off (CL-01 AC3) and the only input to destructive and outbound actions (S7 5.4). T-901 wraps it in the `Classifier` port; T-602c and T-605 call it directly.

## Files

| Action | Path | What |
| --- | --- | --- |
| Create | `backend/crates/domain/src/header_rules.rs` | `HeaderRules`, `UnsubscribeRoute`, scoring, reasons |
| Change | `backend/crates/domain/src/lib.rs` | `pub mod header_rules; pub use header_rules::*;` |

## Types and signatures

```rust
pub struct HeaderRules;                       // stateless; version "header_rules@1" (domain::HEADER_RULES_ID)

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UnsubscribeRoute {
    OneClick(Url),                            // job, method one_click (UN-02)
    Mailto(MailtoTarget),                     // job, method mailto (UN-03)
    ManualLink(Option<Url>),                  // no job: Needs Attention "Open unsubscribe page"; link only when https (UN-04 AC6)
    None,                                     // no DKIM-covered option
}
// Debug for UnsubscribeRoute: hand-written, prints the variant name only.

impl HeaderRules {
    pub fn classify(facts: &HeaderFacts, sender: &SenderKey) -> Classification;
    pub fn unsubscribe_route(facts: &HeaderFacts) -> UnsubscribeRoute;
    pub fn score(facts: &HeaderFacts) -> u8;
}

pub const PERSONAL_HIGH_CONFIDENCE_MAX_SCORE: u8 = 10;   // used by T-103 (GUARD-2)
pub const BULK_REASON_MAX_CHARS: usize = 200;             // S7 Card `bulk_reason`
pub const ESP_NAMES: [(&str, &str); 8] = [                // keys from T-401's ESP_HINTS -> display names
    ("mailchimp", "Mailchimp"), ("sendgrid", "SendGrid"), ("mailgun", "Mailgun"),
    ("amazon_ses", "Amazon SES"), ("salesforce", "Salesforce"), ("hubspot", "HubSpot"),
    ("constant_contact", "Constant Contact"), ("klaviyo", "Klaviyo"),
];
```

## Algorithm

1. **Route** (`unsubscribe_route`), from `facts.list_unsubscribe` only (the DKIM-covered options, S6 6):
   1. `None` options: `UnsubscribeRoute::None`.
   2. `one_click_https` set and its scheme is `https`: `OneClick(url)` (S10 corpus: one-click preferred over mailto).
   3. Else `mailto` set: `Mailto(target)`.
   4. Else `https` set: `ManualLink(Some(url))` when the scheme is `https`, `ManualLink(None)` for plain `http` (S10 corpus row "http:// link": Needs Attention, never fetched; S7 5.8 drops non-https links).
   5. Else `None`.
2. **Score** `[DEFAULT]` (S3 says spike E2 sets thresholds; E2 has not run, so these additive weights stand in and are `[TUNABLE]` by editing one table): start at 0 and add
   - `list_unsubscribe_present`: 30
   - route is `OneClick`, `Mailto` or `ManualLink`: 15
   - `list_id` is some: 15
   - `precedence_bulk`: 20
   - `feedback_id` is some: 10
   - `esp_hint` is some: 10
   - `auto_submitted`: 5
   - `is_reply_or_thread`: subtract 30
   Clamp to 0..=100. A valid one-click header alone gives 45, below the "high" band, as S3 requires ("must not alone push the score to junk").
3. **Class** (`classify`), first rule that holds wins:
   1. `sender.is_empty()`, or (`!facts.from_authenticated` and (`facts.reply_to_mismatch` or `facts.display_name_spoof`)): `Suspect` (S10 corpus: phishing look-alike; T-401: unparsable From).
   2. Route is not `None`: `List` (only DKIM-covered options reach here: CL-01 AC2, UN-02 AC2).
   3. `list_unsubscribe_present` (header present but not covered, or covered but unusable): `BulkNoHeader` (UN-02 AC2, UN-03 AC3, S10 corpus rows "not in DKIM h=" and "DKIM fails").
   4. `precedence_bulk` or `list_id` is some: `BulkNoHeader` (S3: looks bulk, no header).
   5. `auto_submitted` or `sender.is_noreply()` or `esp_hint` is some or `feedback_id` is some: `Notice` (spike E1: headerless senders were account, billing and security notices).
   6. Otherwise `Personal`.
4. **Confidence** `[DEFAULT]`: `Some(score as f32 / 100.0)` for every class (CR-01a uses `bulk_score / 100` as header rules' confidence). `probabilities: None`.
5. **Reason** (plain words, fixed fragments only, joined with ", ", first letter capitalised, at most `BULK_REASON_MAX_CHARS`):
   - Class fragment: `List` "Mailing list"; `BulkNoHeader` "Looks like bulk mail with no unsubscribe header"; `Notice` "Automated notice"; `Personal` "Looks personal"; `Suspect` "Looks suspicious, sender not verified".
   - ESP fragment when `esp_hint` maps through `ESP_NAMES`: "sent through <Name>"; an unknown key gives "sent through a bulk mail service" (never echo the key).
   - Route fragment for `List`: `OneClick` "has one-click unsubscribe"; `Mailto` "unsubscribes by email"; `ManualLink` "unsubscribe needs you to visit their page".
   - `List` with `auto_submitted` true and `precedence_bulk` false: add "may be account updates" `[DEFAULT]` (S10 corpus row "Transactional with one-click": header rules cannot tell marketing from transactional; this is the only hint they have).
   Example: "Mailing list, sent through Mailchimp, has one-click unsubscribe" (S9 3).
6. Everything is a pure function of its arguments: same input, same output, no clock, no randomness.

## Acceptance criteria

| ID | Behaviour (one line) |
| --- | --- |
| CL-01 AC2 | Without a DKIM-covered `List-Unsubscribe` option, header rules never return `list`; a present but uncovered header gives `bulk_no_header` |
| UN-02 AC2 | One-click headers not covered by a passing DKIM signature give `bulk_no_header` and no route |
| UN-02 AC2a | A covered one-click header gives `list` even when the From is not DMARC-aligned (`from_authenticated` false) |
| UN-03 AC3 | A mailto-only header without DKIM cover gives `bulk_no_header` and no route |
| UN-04 AC6 | A covered https link without one-click gives `list` with a manual link route and no job route |
| SW-03 AC5 | A message with no `List-Unsubscribe` header gets no unsubscribe route of any kind |

## Tests that must pass

- `cl_01_ac2_no_covered_option_never_list` (property: any `HeaderFacts` with `list_unsubscribe: None` and any sender never classifies as `List`)
- `cl_01_ac2_present_uncovered_header_is_bulk_no_header` (unit)
- `un_02_ac2_uncovered_one_click_is_bulk_no_header` (unit)
- `un_02_ac2a_list_without_from_alignment` (unit)
- `un_03_ac3_uncovered_mailto_is_bulk_no_header` (unit)
- `un_04_ac6_https_only_gives_manual_link` (unit)
- `sw_03_ac5_no_header_no_route` (property: `list_unsubscribe_present == false` implies route `None`)
- `header_rules_one_click_preferred_over_mailto` (unit)
- `header_rules_plain_http_link_has_no_url` (unit)
- `header_rules_phishing_lookalike_is_suspect` (unit)
- `header_rules_empty_sender_is_suspect` (unit)
- `header_rules_notice_for_auto_submitted_noreply` (unit)
- `header_rules_personal_for_plain_mail` (unit)
- `header_rules_one_click_alone_scores_45` (unit)
- `header_rules_score_always_0_to_100` (property)
- `header_rules_reason_never_contains_header_values` (property: generated `list_id`, `feedback_id` and `esp_hint` strings containing `CANARY` never appear in `bulk_reason`, and the reason is at most 200 characters)
- `header_rules_deterministic` (property: two calls give equal results)

## Edge cases and traps

- Never read `list_unsubscribe_present` as permission to unsubscribe. Only `facts.list_unsubscribe` (DKIM-covered, from T-406) may produce a route.
- Do not use `from_authenticated` to decide `List`; DKIM over the unsubscribe headers is the rule and DMARC is not required (UN-02 AC2a). `from_authenticated` matters for rules and counting (T-104) and for `Suspect` here.
- `esp_hint` is never copied into the reason; map it through `ESP_NAMES` or use the generic phrase. The same goes for `list_id` and `feedback_id`: they are sender-controlled text.
- `Url::scheme()` comparison is case-insensitive after parsing (the `url` crate lower-cases schemes), so compare with `"https"`.
- Do not add Gmail category labels as an input; S3 says they are absent on some accounts.
- Keep every weight and threshold as a named `const` with a `[DEFAULT]` comment, so the E2 spike can change them in one place.
- Property tests need an `Arbitrary`-style strategy for `HeaderFacts`; write one `fn facts_strategy() -> impl Strategy<Value = HeaderFacts>` in a `#[cfg(test)]` module and reuse it. Generate URLs from a small fixed list of `https://unsub.example.com/...` and `http://...example.org/...` values.

## Out of scope

- Clamping model output: T-103. What a reject does with the route: T-105a.
- The `Classifier` trait wrapper and badge plumbing: T-901.
- Building `HeaderFacts` from headers: T-401; DKIM coverage: T-406.

## Done when

- The tests above pass and every required check is green (S10 10.1).
- Definition of done in S10 10.4.
- `bulk_reason` for every S10 5 corpus row type reads as plain Australian English.
