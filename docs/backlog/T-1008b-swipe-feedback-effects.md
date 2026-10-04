# T-1008b: Swipe feedback: animations, sounds, haptics, combo and confetti

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M10 | sonnet | about 350 lines of code plus tests | T-1006a, T-1008a |

**Read only these spec sections:** S9 section 3 "Swipe feedback (GM-02)" (`docs/specs/S9-functional-screens.md`); S2 GM-02 and XC-03 (`docs/specs/S2-v1-acceptance-criteria.md`). Nothing else is needed.

## Goal

Each swipe direction has its own animation, a reject on a high-score card plays a "flush", optional sounds play when the Sounds switch is on, haptics fire where the browser supports vibration, a combo counter appears for fast swiping, and confetti marks every 100 cleared. Reduced motion turns every animation off.

## Files

| Action | Path | What |
| --- | --- | --- |
| Create | `app/lib/platform/sound_player.dart`, `sound_player_web.dart`, `sound_player_stub.dart` | `SoundPlayer` (WebAudio tones, no asset files) |
| Create | `app/lib/platform/haptics.dart`, `haptics_web.dart`, `haptics_stub.dart` | `Haptics` (Vibration API when present) |
| Create | `app/lib/state/feedback_model.dart` | `FeedbackModel`: combo, confetti trigger, which effect to play |
| Create | `app/lib/screens/feed/effects.dart` | Direction animations, flush, combo badge, confetti painter |
| Change | `app/lib/screens/feed/swipeable_card.dart`, `feed_screen.dart` | Play effects |
| Change | `app/lib/copy.dart` | Strings below |
| Create | `app/test/support/fake_sound_player.dart`, `fake_haptics.dart` | Fakes recording calls |
| Create | `app/test/screens/swipe_feedback_test.dart`, `app/test/unit/feedback_model_test.dart` | Tests below |

## Types and signatures

```dart
enum SwipeSound { keep, skip, reject, file, flush }
abstract class SoundPlayer { void play(SwipeSound s); }
abstract class Haptics { bool get supported; void tap(); }

const int kComboSwipes = 5;                          // GM-02 AC3 [TUNABLE]
const Duration kComboWindow = Duration(seconds: 10); // GM-02 AC3 [TUNABLE]
const int kConfettiEvery = 100;                      // GM-02 AC3
const int kFlushScore = 80;                          // [DEFAULT] "three-poop" cards until the badge artwork exists

enum CardEffect { flyRight, flyLeft, flyUp, flyDown, flush }

class FeedbackModel extends ChangeNotifier {
  FeedbackModel({required PlayPrefs prefs, required SoundPlayer sound, required Haptics haptics, DateTime Function()? now});
  int? get combo;                 // null until kComboSwipes within kComboWindow
  bool get confettiDue;           // true once per crossing of a multiple of 100 cleared
  void acknowledgeConfetti();
  CardEffect onSwipe(FeedCard card, SwipeKind kind); // called at swipe time (optimistic)
}
```

## Algorithm

1. `onSwipe`: pick the effect: keep `flyRight`, reject `flyLeft` (or `flush` when `card.bulkScore >= kFlushScore`), file `flyUp`, skip `flyDown`. If `prefs.soundsOn`, `sound.play` the matching sound (GM-02 AC2). If `haptics.supported`, `haptics.tap()`.
2. Combo: keep swipe times in a list, drop those older than `kComboWindow` from `now()`; when the list length is at least `kComboSwipes`, `combo` = list length, else null. Show `Copy.combo(n)` `[DEFAULT]` as a small badge.
3. Confetti: count cleared (reject and file, same rule as T-1008a) in this model for the session; when it reaches 100, 200, and so on, set `confettiDue`. Undo does not un-trigger confetti already shown.
4. Effects widget: fly-off tweens 250 ms toward the swipe direction; flush rotates 360 degrees while scaling to 0 over 400 ms `[DEFAULT]`; confetti is a `CustomPainter` with 60 particles over 1.5 s `[DEFAULT]`, no package.
5. Reduced motion (GM-02 AC4, XC-03): when `MediaQuery.disableAnimationsOf(context)` is true, no fly-off, flush or confetti animation runs; the card simply leaves, and confetti is replaced by a static text `Copy.clearedMilestone(n)` `[DEFAULT]` for 2 seconds. Sounds and the combo badge still follow their own rules.
6. Web `SoundPlayer`: one `AudioContext` created lazily on first play (after a user gesture); each sound a 120 ms oscillator tone at a fixed frequency per sound `[DEFAULT]`. Web `Haptics.supported` checks that `navigator.vibrate` exists; `tap()` calls `navigator.vibrate(15)`.

Copy: `combo(n)` `[DEFAULT]` "Combo $n"; `clearedMilestone(n)` `[DEFAULT]` "${formatCount(n)} cleared".

## Acceptance criteria

| ID | Behaviour (one line) |
| --- | --- |
| GM-02 AC1 | Each direction plays its own animation; a reject on a high-score card plays the flush |
| GM-02 AC2 | Sounds are off by default and play only when switched on; haptics only where the Vibration API exists |
| GM-02 AC3 | A combo counter appears after 5 swipes within 10 seconds; confetti plays at every 100 cleared |
| GM-02 AC4 | All effects respect reduced motion |
| XC-03 | Animations respect the reduced-motion setting |

## Tests that must pass

- `'GM-02 AC1 each direction plays its own animation'` (widget, effect key per direction)
- `'GM-02 AC1 reject on a high bulk score card plays the flush'` (widget)
- `'GM-02 AC2 no sound plays while Sounds is off'` (widget)
- `'GM-02 AC2 a sound plays per swipe when Sounds is on'` (widget)
- `'GM-02 AC2 haptics only when the Vibration API exists'` (unit)
- `'GM-02 AC3 combo appears after 5 swipes within 10 seconds'` (unit, injected clock)
- `'GM-02 AC3 no combo when swipes are spread out'` (unit)
- `'GM-02 AC3 confetti at 100 cleared'` (widget)
- `'GM-02 AC4 reduced motion runs no animation'` (widget, `disableAnimations: true`, `tester.hasRunningAnimations` false after one pump)
- `'XC-03 reduced motion shows the static milestone instead of confetti'` (widget)

## Edge cases and traps

- Never create an `AudioContext` before a user gesture (browsers block it) and never play anything when Sounds is off.
- No audio files or third-party packages; tones only (keeps the CSP simple, T-006).
- Use the injected clock for the combo window; never `DateTime.now()` in the model.
- Effects must not delay the swipe call or the next card; they are visual only.
- No effect state is persisted.

## Out of scope

- Badge artwork and real sound design (visual design phase).

## Done when

- The tests above pass and every required check is green (S10 10.1).
- Definition of done in S10 10.4.
