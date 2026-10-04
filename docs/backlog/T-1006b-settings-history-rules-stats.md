# T-1006b: Settings: History, Rules and Stats

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M10 | sonnet | about 400 lines of code plus tests | T-1004, T-1006a |

**Read only these spec sections:** S9 sections 7.2, 7.3, 7.4 and 9 (`docs/specs/S9-functional-screens.md`); S7 sections 5.7 (API-RULE-1, API-RULE-3, API-RULE-4) and 5.9 (API-HIST-1, API-STAT-1) and the `Rule`, `HistoryEntry`, `Achievement` schemas in `docs/specs/S7-api-contract.openapi.yaml`; S2 ST-01, ST-02, SR-01 AC4, GM-05 AC2 and AC3, GM-06 AC1 and AC3. Nothing else is needed.

## Goal

Three read-mostly Settings screens: History (every automated action, filtered, linking to its rule), Rules (reject list, blocked people, filing rules with switches and delete) and Stats (totals plus the achievements list). This task also owns the achievements catalogue that T-1008a uses.

## Files

| Action | Path | What |
| --- | --- | --- |
| Create | `app/lib/api/models/rule.dart` | `Rule`, `RuleKind`, `RuleMatch` |
| Create | `app/lib/api/models/history.dart` | `HistoryEntry`, `HistoryPage`, `HistoryFilter` |
| Create | `app/lib/api/models/stats.dart` | `Stats` (reuses `Achievement` from T-1002b) |
| Create | `app/lib/achievements.dart` | `AchievementInfo`, `kAchievements`, `achievementTitle` |
| Create | `app/lib/state/rules_model.dart`, `history_model.dart`, `stats_model.dart` | View models |
| Create | `app/lib/screens/settings/history_screen.dart`, `rules_screen.dart`, `rule_sheet.dart`, `stats_screen.dart` | Screens |
| Change | `app/lib/screens/settings/settings_entries.dart` | Add History, Rules, Stats |
| Change | `app/lib/api/api_client.dart`, `http_api_client.dart`, `fake_api_client.dart` | Methods below |
| Change | `app/lib/copy.dart` | Strings below |
| Create | `app/test/screens/history_test.dart`, `rules_test.dart`, `stats_test.dart` | Tests below |

## Types and signatures

```dart
enum RuleKind { rejectList, blockPerson, file }  // wire reject_list, block_person, file
class RuleMatch { final String senderAddress; final String? listId; }
class Rule { final String ruleId; final RuleKind kind; final RuleMatch match; final String? categoryId;
             final bool enabled; final DateTime createdAt; final int timesApplied; final int? yearlyRate; }

enum HistoryFilter { all, unsubscribes, ruleActions, filing }  // wire all, unsubscribes, rule_actions, filing
class HistoryEntry { final String entryId, mailboxId, senderDisplay, action, outcome; final DateTime at; final String? ruleId; }
class HistoryPage { final List<HistoryEntry> entries; final String? nextCursor; }
class Stats { final int emailsTriaged, sendersUnsubscribed, unsubscribesConfirmed, mailStoppedPerYear; final List<Achievement> achievements; }

abstract class ApiClient {
  Future<List<Rule>> listRules({RuleKind? kind});                    // GET /rules
  Future<Rule> setRuleEnabled(String ruleId, bool enabled);          // PATCH /rules/{id} {enabled}
  Future<void> deleteRule(String ruleId);                            // DELETE /rules/{id}
  Future<HistoryPage> listHistory({HistoryFilter filter = HistoryFilter.all, String? cursor, int limit = 20});
  Future<Stats> getStats();                                          // GET /stats
}

class AchievementInfo { const AchievementInfo(this.id, this.title); final String id; final String title; }
const List<AchievementInfo> kAchievements = [
  AchievementInfo('first_unsubscribe', 'First unsubscribe'),        // id from S7 example
  AchievementInfo('senders_silenced_100', '100 senders silenced'),
  AchievementInfo('year_cleared', 'A year cleared'),
  AchievementInfo('cleared_1000', '1,000 cleared'),
  AchievementInfo('first_filing_category', 'First filing category'),
  AchievementInfo('first_blocked_person', 'First blocked person'),
  AchievementInfo('ten_unsubscribes_in_round', 'Ten unsubscribes in a round'), // IDs match T-109 AchievementId (snake_case)
];
String? achievementTitle(String id); // null for unknown IDs
```

## Algorithm

1. **History** (ST-01): segmented control with "All", "Unsubscribes", "Rule actions", "Filing"; changing it reloads with that filter. Rows newest first: `formatDateTime(at)`, mailbox address, `senderDisplay`, action words and outcome words (tables below). Load more near the end of the list. Empty `Copy.historyEmpty` `[DEFAULT]`.
2. Load all rules once when History opens (`listRules()`). For an entry with `ruleId` found in that list, show `Copy.yearlyStopped(rate)` or, when `yearlyRate == null`, `Copy.yearlyUnknown` (GM-05 AC2, AC3). Tapping such an entry opens `RuleSheet` with the rule's match and an on or off switch (ST-01 AC2).
3. **Rules** (S9 7.3): three sections "Reject list", "Blocked people", "Filing rules" `[DEFAULT headings]`. Each row: sender address, list ID when present, for filing rules the category name (from `listCategories()` of T-1004), and `Copy.actedOn(timesApplied)`. A switch calls `setRuleEnabled` (SR-01 AC4); revert the switch and show `Copy.actionFailed` on error. Delete asks `Copy.deleteRuleQuestion` then calls `deleteRule`.
4. **Stats** (ST-02): four labelled figures with `formatCount`: `Copy.emailsTriaged`, `Copy.sendersUnsubscribed`, `Copy.unsubscribesConfirmed`, and the mail-stopped line `Copy.mailStoppedTotal(n)`. Below, every entry of `kAchievements`: unlocked ones show the title and `formatDate(unlockedAt)`; locked ones are drawn at 40% opacity with Semantics "<title>, locked" (ST-02 AC3). Unknown unlocked IDs from the server are listed at the end with `Copy.achievementUnknown` `[DEFAULT]`.

History words `[DEFAULT]`: actions `trashed_by_rule` "Trashed by rule", `unsubscribe` "Unsubscribe", `filed` "Filed", `filed_by_rule` "Filed by rule", `blocked` "Blocked", `reported_spam` "Reported as spam"; outcomes `sent` "Sent", `needs_attention` "Needs attention", `failed` "Failed", `cancelled` "Cancelled", `expired` "Expired", `done` "Done".

Other copy: `history` "History", `rules` "Rules", `stats` "Stats", `historyEmpty` `[DEFAULT]` "Nothing here yet.", `yearlyStopped(n)` `[DEFAULT]` "About $n emails a year stopped", `yearlyUnknown` `[DEFAULT]` "Emails a year stopped: unknown", `actedOn(n)` `[DEFAULT]` "Acted on $n messages", `deleteRuleQuestion` `[DEFAULT]` "Delete this rule? It stops acting on new mail.", `emailsTriaged` `[DEFAULT]` "Emails triaged", `sendersUnsubscribed` `[DEFAULT]` "Senders unsubscribed", `unsubscribesConfirmed` `[DEFAULT]` "Unsubscribes confirmed working", `mailStoppedTotal(n)` "About $n emails a year stopped" (S2 ST-02 AC2 example), `achievementUnknown` `[DEFAULT]` "Achievement".

## Acceptance criteria

| ID | Behaviour (one line) |
| --- | --- |
| ST-01 AC1 | History lists automated actions newest first with time, sender and mailbox |
| ST-01 AC2 | A rule-driven entry opens its rule, which can be switched off |
| SR-01 AC4 | Switching a rule off sends `enabled: false` |
| GM-05 AC2 | History shows the per-sender yearly figure; Stats shows the total |
| GM-05 AC3 | A null yearly figure shows as unknown |
| ST-02 AC1 | Stats shows emails triaged, senders unsubscribed and unsubscribes confirmed |
| ST-02 AC2 | Stats shows the mail stopped total |
| ST-02 AC3 | Achievements show unlocked with dates and locked greyed |
| GM-06 AC3 | An unlocked achievement appears in Stats |

## Tests that must pass

- `'ST-01 AC1 History lists entries newest first with time, sender and mailbox'` (widget)
- `'ST-01 AC1 filters send the filter value'` (widget)
- `'ST-01 AC2 a rule-driven entry opens its rule and the switch turns it off'` (widget)
- `'SR-01 AC4 switching a rule off sends enabled false'` (widget)
- `'GM-05 AC2 History shows the per-sender yearly figure'` (widget)
- `'GM-05 AC3 unknown yearly figure shows as unknown'` (widget)
- `'ST-02 AC1 Stats shows triaged, unsubscribed and confirmed'` (widget)
- `'ST-02 AC2 Stats shows the mail stopped total'` (widget)
- `'ST-02 AC3 achievements unlocked with dates and locked greyed'` (widget)
- `'GM-06 AC3 unlocked achievement appears in Stats'` (widget)
- `'s9_rules_lists_three_kinds_with_counts'` (widget)
- `'s9_rules_delete_confirms_then_deletes'` (widget)
- `'s9_rules_switch_error_reverts'` (widget)
- `'s9_history_empty'` (widget)
- `'achievementTitle covers every catalogue ID and returns null for unknown'` (unit)
- `'XC-03 History, Rules and Stats controls are labelled'` (widget)

## Edge cases and traps

- The achievement IDs must match T-109's `AchievementId` wire strings; if T-109 changed them when it landed, follow T-109 and say so in the PR.
- Never show a rule's `list_id` or address in a Semantics label longer than the visible text; plain `Text` only.
- `yearly_rate` null is "unknown", not 0, and is left out of nothing on screen (the server already ignores it in the total).
- Rule switches are optimistic: revert on failure.
- No browser storage for any of this.

## Out of scope

- Creating rules (T-1002b block prompt, T-1003 keep prompt); achievement celebrations on the Feed (T-1008a).

## Done when

- The tests above pass and every required check is green (S10 10.1).
- Definition of done in S10 10.4.
