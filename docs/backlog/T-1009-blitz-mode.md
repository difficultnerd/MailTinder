# T-1009: Blitz mode

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M10 | sonnet | about 300 lines of code plus tests | T-1008a |

**Read only these spec sections:** S9 sections 3 ("Blitz entry") and 3.1 (`docs/specs/S9-functional-screens.md`); S2 GM-07 and PB-01 AC1 (`docs/specs/S2-v1-acceptance-criteria.md`). Nothing else is needed.

## Goal

A "Blitz" button on the Feed starts a 60-second round with a visible timer and score. Personal cards are left out of the round, swipes and undo work exactly as usual, block prompts wait until the round ends, and the round ends with a results card. Nothing about the round outlives it.

## Files

| Action | Path | What |
| --- | --- | --- |
| Create | `app/lib/state/blitz_model.dart` | `BlitzModel`, `BlitzStatus`, `BlitzResult` |
| Create | `app/lib/screens/feed/blitz_bar.dart` | Timer, score and End round |
| Create | `app/lib/screens/feed/blitz_results_card.dart` | Results card |
| Change | `app/lib/state/feed_model.dart` | `setHiddenFilter(bool Function(FeedCard)?)` |
| Change | `app/lib/screens/feed/feed_screen.dart` | Blitz button and wiring |
| Change | `app/lib/copy.dart` | Strings below |
| Create | `app/test/screens/blitz_test.dart` | Tests below |

## Types and signatures

```dart
const Duration kBlitzLength = Duration(seconds: 60); // GM-07 AC1

enum BlitzStatus { idle, running, paused, ended }
class BlitzResult { final int score, cleared, kept, filed; }

class BlitzModel extends ChangeNotifier {
  BlitzModel({required FeedModel feed, required SwipeController swipes});
  BlitzStatus get status;
  Duration get remaining;
  int get score;
  BlitzResult? get result;
  void start();
  void end();            // End round, or the timer reaching zero
  void pause();          // tab hidden
  void resume();         // tab visible
  void dismissResults(); // then held block prompts are shown in turn
}

// FeedModel (change): cards for which the filter returns true are skipped over when picking
// `current` and `next`, but keep their place in the queue; null shows everything again.
void setHiddenFilter(bool Function(FeedCard card)? hidden);
```

## Algorithm

1. "Blitz" button on the Feed (Semantics `Copy.blitzSemantics`) calls `start()`: status `running`, `remaining = kBlitzLength`, score 0, `feed.setHiddenFilter((c) => c.messageClass == MessageClass.personal)` (GM-07 AC2), `swipes.holdPrompts = true` (GM-07 AC4).
2. A `Timer.periodic(1 second)` decrements `remaining` while `running`. At zero call `end()`.
3. Score `[DEFAULT]`: +1 for each acked keep, reject or file in the round, 0 for skip; an undo of one of those subtracts 1. Count cleared, kept and filed the same way as T-1008a. Listen to `swipes.events`.
4. Swipes, the unsubscribe delay toast and undo are untouched (GM-07 AC3): Blitz only filters and counts.
5. Pause: use `AppLifecycleListener` (`onHide` pauses, `onShow` resumes) so a hidden tab stops the timer (S9 "interrupted").
6. `end()`: cancel the timer, status `ended`, `feed.setHiddenFilter(null)` (personal cards come back in their places), `swipes.holdPrompts = false`, build `result`. Show the results card. On dismiss, show each of `swipes.takeHeldPrompts()` in turn through the T-1002b block dialog, then status `idle` and clear `result` (GM-07 AC5).
7. Bar during the round: `Copy.blitzTimer(remaining)` as "0:45", `Copy.blitzScore(score)`, and "End round".

Copy `[DEFAULT]` unless marked: `blitz` "Blitz"; `blitzSemantics` "Start a 60-second Blitz round"; `blitzTimer` "m:ss"; `blitzScore(n)` "Score $n"; `endRound` "End round" (S9); results card title "Blitz over", rows "Score", "Cleared", "Kept", "Filed", button "Done".

## Acceptance criteria

| ID | Behaviour (one line) |
| --- | --- |
| GM-07 AC1 | Blitz starts a 60-second round with a visible timer and score that ends at zero with a results card |
| GM-07 AC2 | Personal cards are not shown during the round |
| GM-07 AC3 | The unsubscribe delay toast and undo work exactly as outside Blitz |
| GM-07 AC4 | Block prompts are held until the round ends, then shown in turn |
| GM-07 AC5 | Nothing about the round is kept beyond the results card in memory |

## Tests that must pass

- `'GM-07 AC1 Blitz starts a 60 second round with timer and score'` (widget)
- `'GM-07 AC1 round ends at zero with a results card'` (widget, `tester.pump(Duration(seconds: 60))`)
- `'GM-07 AC2 personal cards are not shown during the round'` (widget)
- `'GM-07 AC2 personal cards return after the round'` (widget)
- `'GM-07 AC3 reject toast states the delay and undo works in a round'` (widget)
- `'GM-07 AC4 block prompts are held until the round ends then shown in turn'` (widget)
- `'GM-07 AC5 nothing about the round survives dismissing the results'` (widget)
- `'s9_blitz_running'` (widget)
- `'s9_blitz_ended'` (widget)
- `'s9_blitz_interrupted pauses when the tab is hidden'` (widget, `AppLifecycleState.hidden`)
- `'End round ends the round early'` (widget)
- `'XC-03 Blitz controls are labelled'` (widget)

## Edge cases and traps

- Hiding personal cards is client-side only: never send a skip for them.
- Always reset `holdPrompts` and the filter in `end()`, including when the Feed is disposed mid-round (call `end()` in `dispose`).
- Cancel the timer on end and dispose; a leaked periodic timer fails widget tests.
- No score, timer or result in browser storage or on the server.

## Out of scope

- Leaderboards or saved high scores (not in v1).

## Done when

- The tests above pass and every required check is green (S10 10.1).
- Definition of done in S10 10.4.
