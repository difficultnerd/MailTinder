# T-608: Rules endpoints and block prompt decline

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M6 | sonnet | about 300 lines of code plus tests | T-605, T-607a |

**Read only these spec sections:** S7 section 5.7 (API-RULE-1 to API-RULE-5) in `docs/specs/S7-api-contract.md`; `Rule`, `RuleKind` schemas and the `/rules`, `/rules/{rule_id}`, `/block-prompts/decline` paths in `docs/specs/S7-api-contract.openapi.yaml`; S2 SR-01 AC4, PB-01 AC2 and AC3, FL-04 AC2, ST-01 AC2, GM-05 AC2. Nothing else is needed.

## Goal

The Rules screen and the block and keep prompts get their backend: list rules, create a block rule from a sealed prompt reference or a filing rule from a message, switch a rule on or off, delete it, and decline a block prompt for 90 days.

## Files

| Action | Path | What |
| --- | --- | --- |
| Create | `backend/crates/api/src/routes/rules.rs` | Five handlers and DTOs |
| Create | `backend/crates/api/src/services/rules.rs` | Rule service |
| Change | `backend/crates/api/src/routes/mod.rs` | Mount routes |
| Create | `backend/crates/api/tests/rules.rs` | Service integration tests |

## Types and signatures

```rust
// Used from earlier tasks: SortRule, RuleKind, block_person_for, file_for, SenderStats::decline_block_prompt (T-104);
// StoredRule, HistoryEntry (T-602b); BlockPromptRef (T-605); SealedTokens, TokenType::PromptRef (T-303).

#[derive(Serialize)]
pub struct RuleDto { pub rule_id: Uuid, pub kind: &'static str, pub r#match: RuleMatchDto, pub category_id: Option<Uuid>,
    pub enabled: bool, pub created_at: OffsetDateTime, pub times_applied: u64, pub yearly_rate: Option<u32> }
#[derive(Serialize)] pub struct RuleMatchDto { pub sender_address: String, pub list_id: Option<String> }

#[derive(Deserialize)] #[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CreateRuleDto {
    BlockPerson { prompt_ref: String },
    File { mailbox_id: Uuid, message_id: String, category_id: Uuid },
}
#[derive(Deserialize)] #[serde(deny_unknown_fields)] pub struct PatchRuleDto { pub enabled: bool }
#[derive(Deserialize)] #[serde(deny_unknown_fields)] pub struct DeclineDto { pub prompt_ref: String }

pub const NS_RULE_FROM_PROMPT: Uuid = Uuid::from_u128(0x6d74_7275_0000_4000_8000_000000000001); // [DEFAULT]
```

`#[serde(tag = "kind")]` with `deny_unknown_fields` is not supported together on internally tagged enums in every serde version; if it fails, deserialise into a flat struct with all optional fields and validate the two shapes by hand. Either way unknown fields must give `400`.

## Algorithm

1. API-RULE-1: optional `kind` filter (`reject_list`, `block_person`, `file`; anything else `400`). Map each `StoredRule`; `match.list_id` is the List-Id or `null`; the Feedback-ID key stays internal. Sort by `created_at` descending. `yearly_rate` and `times_applied` come from the stored rule (GM-05 AC2).
2. API-RULE-2 `block_person` (PB-01 AC2): open `prompt_ref` as `TokenType::PromptRef` (failure `400 invalid_request`). `rule_id = v5(NS_RULE_FROM_PROMPT, user ++ swipe_id)`, so a retry finds the same rule. If an enabled `block_person` rule for that sender exists, return it with `201`. Else `block_person_for(sender, rule_id, now, Some(swipe_id))`; push with `times_applied 0`, `yearly_rate None`; History `{ entry_id: rule_id, action: blocked, outcome: done, rule_id }`; `totals.people_blocked += 1`, `senders_silenced += 1`. Return `201`.
3. API-RULE-2 `file` (FL-04 AC2): mailbox must be the user's (`404`); category must exist (`404`); re-read the message with `get_meta` (`NotFound`: `409 message_changed`); the sender comes from that fresh read, never the client. `rule_id = Rng::uuid_v4`. Reuse an identical enabled rule if present. `file_for(sender, category, rule_id, now)`. Return `201`. The rule acts from the next Feed load (T-609).
4. API-RULE-3 (SR-01 AC4, ST-01 AC2): unknown ID `404`; set `enabled`; return `200` with the rule.
5. API-RULE-4: unknown ID `404`; remove it; `204`. History entries that name it keep their `rule_id` (the app shows "rule deleted").
6. API-RULE-5 (PB-01 AC3): open `prompt_ref` (`400` on failure); `sender_stats[sender].decline_block_prompt(now, tunables)` (90 days `[TUNABLE]`); `204`.

## Acceptance criteria

| ID | Behaviour (one line) |
| --- | --- |
| SR-01 AC4 | A rule switched off stops matching from then on |
| PB-01 AC2 | Confirming the prompt creates a block rule listed in History |
| PB-01 AC3 | Declining suppresses the prompt for that sender for 90 days |
| FL-04 AC2 | Accepting the keep prompt creates a filing rule from the re-read message |
| ST-01 AC2 | A rule linked from History can be switched off |
| GM-05 AC2 | Each rule shows its yearly estimate, or null when unknown |

## Tests that must pass

- `sr_01_ac4_disabled_rule_not_applied` (service integration with T-609; before T-609 merges, assert `enabled: false` is stored and returned)
- `pb_01_ac2_block_rule_from_prompt_ref` (service integration: rule present, History `blocked` entry present)
- `pb_01_ac2_retry_creates_one_rule` (service integration)
- `pb_01_ac3_decline_suppresses_for_90_days` (service integration: rejects on day 89 give no prompt, day 91 can prompt again)
- `fl_04_ac2_file_rule_from_message` (service integration)
- `st_01_ac2_switch_off_rule` (service integration)
- `gm_05_ac2_rule_lists_yearly_rate` (service integration, one known and one null)
- `asvs_v9_2_2_classification_token_as_prompt_ref_refused` (service integration: `400`)
- `asvs_v8_2_2_other_users_rule_not_found` (service integration)

## Edge cases and traps

- A `block_person` rule can only come from a server-sealed prompt; never accept a sender address from the client.
- The `file` form takes the sender from a fresh provider read, never from the request.
- `reject_list` rules are created only by API-SW-1; `kind: "reject_list"` on API-RULE-2 is `400`.
- Every write is one `UserStateStore::update`; provider reads happen before it.
- No sender address or List-Id in logs.

## Out of scope

- Applying rules to mail at Feed load: T-609.

## Done when

- The tests above pass and every required check is green (S10 10.1).
- Definition of done in S10 10.4.
