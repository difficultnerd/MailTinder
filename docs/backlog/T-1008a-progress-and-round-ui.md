# T-1008a: Progress UI: inbox meter, levels, bosses, round card and celebrations

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M10 | sonnet | about 400 lines of code plus tests | T-1002b, T-1006b |

**Read only these spec sections:** S9 section 3 ("Progress elements", "Overlays" items end-of-round, level complete, achievement, boss defeated) (`docs/specs/S9-functional-screens.md`); S7 sections 5.3 (API-PROG-1) and 5.5 (`achievements_unlocked`, `boss_defeated`) and the Card `boss` field in 5.4 (`docs/specs/S7-api-contract.md`); S2 GM-01, GM-03, GM-04, GM-06 AC3, GM-08. Nothing else is needed.

## Goal

The Feed shows the inbox meter, the level banner and the boss banner, and plays the short overlays: level complete, achievement unlocked, boss defeated and the end-of-round card. Round totals (GM-03) live only in browser memory and are owned here.

## Files

| Action | Path | What |
| --- | --- | --- |
| Create | `app/lib/api/models/progress.dart` | `Progress`, `Level` |
| Create | `app/lib/state/progress_model.dart` | `ProgressModel` |
| Create | `app/lib/state/round_tracker.dart` | `RoundTracker`, `RoundTotals` |
| Create | `app/lib/screens/feed/progress_header.dart` | Meter, level banner, boss banner |
| Create | `app/lib/screens/feed/celebrations.dart` | Level complete, achievement, boss defeated overlays |
| Create | `app/lib/screens/feed/round_card.dart` | End-of-round card |
| Change | `app/lib/screens/feed/feed_screen.dart` | Wire the above to `SwipeController.events` and the tab selection |
| Change | `app/lib/api/api_client.dart`, `http_api_client.dart`, `fake_api_client.dart` | `getProgress` |
| Change | `app/lib/copy.dart` | Strings below |
| Create | `app/test/screens/progress_test.dart`, `app/test/unit/round_tracker_test.dart` | Tests below |

## Types and signatures

```dart
class Level { final int year; final int remaining; }
class Progress { final int inboxCount; final List<MailboxError> mailboxErrors; final Level? level; }

abstract class ApiClient { Future<Progress> getProgress(); } // GET /progress

const int kProgressEverySwipes = 10;   // GM-01 AC2 [TUNABLE]
const int kRoundSwipes = 50;           // GM-03 AC1 [TUNABLE]

class ProgressModel extends ChangeNotifier {
  ProgressModel({required ApiClient api});
  int? get inboxCount; int? get startCount; bool get partial; Level? get level;
  Level? takeCompletedLevel();          // set when the level year moves to an older year
  Future<void> load();                  // on Feed open
  void onSwipeAcked();                  // reloads every kProgressEverySwipes acks
}

class RoundTotals { int cleared = 0, kept = 0, filed = 0, unsubscribed = 0, blocked = 0; }
class RoundTracker extends ChangeNotifier {  // memory only (GM-03 AC2)
  RoundTotals get totals; int get swipesSinceCard; bool get cardDue;
  void onEvent(SwipeEvent e);            // from SwipeController.events (T-1002b)
  void markLeftFeed();                   // card due when the Feed is next visible
  void markDividerReached();
  RoundTotals takeCard();                // returns totals and starts a new round
}
```

## Algorithm

1. **Meter** (GM-01): `load()` on Feed open; the first success sets `startCount`. `onSwipeAcked()` reloads after every 10 acks. Text `Copy.meter(count, change)`: `formatCount(count)` then ", down N today" when count fell, ", up N today" `[DEFAULT]` when it rose, ", no change today" `[DEFAULT]` when equal. When `mailboxErrors` is not empty append "*" with Semantics `Copy.meterPartial` `[DEFAULT]` (GM-01 AC2).
2. **Level banner** (GM-04 AC1): when `level != null`, show `Copy.level(year, remaining)`. When a reload returns an older `year` than before, store the old level as completed; the Feed shows the level-complete overlay `Copy.levelComplete(old, new)` `[DEFAULT]` with "Continue" (GM-04 AC2).
3. **Boss banner** (GM-08 AC2): when the current card has `boss`, show `senderName` and a `LinearProgressIndicator` with value `remaining / firstSeenRemaining` (first value seen for that sender address this session, held in memory), Semantics `Copy.bossLabel(name, remaining)` `[DEFAULT]`.
4. **Celebrations** from `SwipeAcked` events: each `achievementsUnlocked` item shows `Copy.achievementUnlocked(title)` with the title from `achievementTitle` (T-1006b; unknown IDs use the plain prefix) (GM-06 AC3); `bossDefeated` true shows `Copy.bossDefeated(name)` `[DEFAULT]` (GM-08 AC3). Overlays queue, each shows for 2 seconds or until tapped `[DEFAULT]`.
5. **Round totals** (GM-03): on `SwipeAcked`: reject or file adds 1 to `cleared` `[DEFAULT: cleared = messages that left the inbox]`; keep adds to `kept`; file adds to `filed`; outcome `trashed_unsubscribe_queued` adds to `unsubscribed`; `BlockAccepted` adds to `blocked`. `SwipeUndone` reverses the same counts. Every ack or undo counts toward `swipesSinceCard`.
6. **Round card due** (GM-03 AC1) when `swipesSinceCard >= kRoundSwipes`, when the divider item becomes current, or after the Feed tab stops being selected (listen to T-007's tab selection; show the card the next time the Feed is visible). The card shows the five totals with labels and "Keep going"; dismissing calls `takeCard()`.
7. Nothing in this task writes to browser storage, the server or logs; a new app instance starts every total at zero (GM-03 AC2).

Copy (S2 and S9 examples verbatim where given): `meter(c, d)` "12,431, down 214 today" pattern; `level(y, r)` "Level $y: ${formatCount(r)} left"; `levelComplete(o, n)` `[DEFAULT]` "Level $o complete. Level $n unlocked."; `achievementUnlocked(t)` `[DEFAULT]` "Achievement unlocked: $t"; `bossDefeated(n)` `[DEFAULT]` "Boss defeated: $n"; `bossLabel(n, r)` `[DEFAULT]` "Boss $n: $r left"; `meterPartial` `[DEFAULT]` "Some mailboxes didn't answer"; round card `[DEFAULT]` title "Round over", rows "Cleared", "Kept", "Filed", "Senders unsubscribed", "Senders blocked", button "Keep going".

## Acceptance criteria

| ID | Behaviour (one line) |
| --- | --- |
| GM-01 AC1 | The meter shows the total and the change since the session started |
| GM-01 AC2 | Progress loads on open and after every 10 swipes; a failing mailbox shows a marker |
| GM-03 AC1 | The round card shows cleared, kept, filed, unsubscribed and blocked after 50 swipes, at the divider, or after leaving the Feed |
| GM-03 AC2 | Round totals live in memory only and start at zero in a new app instance |
| GM-04 AC1 | The level banner shows the year and mail left |
| GM-04 AC2 | Level complete shows when the level moves to an older year |
| GM-06 AC3 | An unlock shows a short celebration on the Feed |
| GM-08 AC2 | A boss card shows a banner with a health bar |
| GM-08 AC3 | `boss_defeated` plays the boss defeated celebration |

## Tests that must pass

- `'GM-01 AC1 meter shows total and change today'` (widget)
- `'GM-01 AC2 progress loads on open and after every 10 swipes'` (widget)
- `'GM-01 AC2 failing mailbox shows the total with a marker'` (widget)
- `'GM-03 AC1 round card after 50 swipes shows the five totals'` (widget)
- `'GM-03 AC1 round card at the up to date divider'` (widget)
- `'GM-03 AC1 round card after leaving the Feed'` (widget)
- `'GM-03 AC2 round totals start at zero in a new app instance'` (widget)
- `'GM-04 AC1 level banner shows year and mail left'` (widget)
- `'GM-04 AC2 level complete shows when the year changes'` (widget)
- `'GM-06 AC3 achievement celebration on the Feed'` (widget)
- `'GM-08 AC2 boss banner shows a health bar'` (widget)
- `'GM-08 AC3 boss defeated celebration'` (widget)
- `'undo reverses round counts'` (unit)
- `'XC-03 progress elements have semantics labels'` (widget)

## Edge cases and traps

- `level` is null until new mail is cleared; show no banner then, not "Level null".
- Count acks, not optimistic swipes, so a failed swipe does not move the meter schedule or round totals.
- Do not persist `startCount`, boss first-seen values or round totals anywhere.
- Unknown achievement IDs must not crash.
- Overlays must not block swipe buttons for longer than their 2 seconds.

## Out of scope

- Animations, sounds, haptics, combo and confetti (T-1008b); Blitz (T-1009).

## Done when

- The tests above pass and every required check is green (S10 10.1).
- Definition of done in S10 10.4.
