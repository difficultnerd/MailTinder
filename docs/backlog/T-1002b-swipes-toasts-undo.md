# T-1002b: Swipes, buttons, toasts, undo and the block prompt

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M10 | sonnet | about 450 lines of code plus tests | T-1002a |

**Read only these spec sections:** S9 section 3 (gestures table, feedback, overlays: block prompt, States "Action failed" and "Message changed", Undo states) and section 9 (`docs/specs/S9-functional-screens.md`); S7 section 5.5 (API-SW-1 rules for optimistic swipes, outcome table, API-SW-2), API-RULE-2 `block_person` form and API-RULE-5 in 5.7 (`docs/specs/S7-api-contract.md`); S2 SW-01 to SW-05, PB-01, FD-04, XC-03, XC-04. Nothing else is needed.

## Goal

Every card can be swiped in four directions or handled by the matching button, swipes are optimistic and sent one at a time, a toast names what happened with Undo, Undo works back through the session's swipes, failures put the card back with the S9 message, and the block prompt appears when the server asks.

## Files

| Action | Path | What |
| --- | --- | --- |
| Create | `app/lib/api/models/swipe.dart` | `SwipeKind`, `SwipeRequest`, `SwipeResult`, `BlockPrompt`, `Achievement`, `UndoResult` |
| Create | `app/lib/state/swipe_controller.dart` | `SwipeController`, `UndoEntry`, `SwipeEvent` |
| Create | `app/lib/state/id_generator.dart` | `IdGenerator` (UUID v4 from `Random.secure()`) |
| Create | `app/lib/screens/feed/swipeable_card.dart` | Gesture detection and fly-off |
| Create | `app/lib/screens/feed/swipe_buttons.dart` | Reject, Skip, File, Keep, Undo |
| Create | `app/lib/screens/feed/block_prompt.dart` | Block dialog |
| Change | `app/lib/screens/feed/feed_screen.dart` | Wire controller, buttons, toast |
| Change | `app/lib/api/api_client.dart`, `http_api_client.dart`, `fake_api_client.dart` | `swipe`, `undo`, `createBlockRule`, `declineBlockPrompt` |
| Change | `app/lib/copy.dart` | Strings below |
| Create | `app/test/screens/feed_swipes_test.dart`, `app/test/unit/swipe_controller_test.dart` | Tests below |

## Types and signatures

```dart
enum SwipeKind { keep, skip, reject, file }   // wire: keep, skip, reject, file
class SwipeRequest {
  final String mailboxId, messageId, classificationToken;
  final SwipeKind kind;
  final String? categoryId, newCategoryName;  // file only: exactly one is set
  Map<String, dynamic> toJson();
}
class SwipeResult {
  final String outcome;                       // S7 outcome string, kept as received
  final DateTime? unsubscribeDueAt;
  final CategoryRef? filedCategory;
  final String undoToken;
  final List<BlockPrompt> prompts;
  final List<Achievement> achievementsUnlocked;
  final bool bossDefeated;
}
class BlockPrompt { final String promptRef; final String senderName; }
class Achievement { final String achievementId; final DateTime unlockedAt; }
class UndoResult { final bool restored; final bool unsubscribeAlreadySent; }

abstract class ApiClient {
  Future<SwipeResult> swipe(SwipeRequest req, {required String idempotencyKey}); // POST /swipes
  Future<UndoResult> undo(String undoToken);                                     // POST /swipes/undo
  Future<void> createBlockRule(String promptRef);  // POST /rules {kind: block_person, prompt_ref}
  Future<void> declineBlockPrompt(String promptRef); // POST /block-prompts/decline
}

class FilingChoice { final String? categoryId; final String? newCategoryName; }
typedef FilingSheetLauncher = Future<FilingChoice?> Function(BuildContext context, FeedCard card);

sealed class SwipeEvent {}
final class SwipeAcked extends SwipeEvent { SwipeAcked(this.card, this.kind, this.result); final FeedCard card; final SwipeKind kind; final SwipeResult result; }
final class SwipeUndone extends SwipeEvent { SwipeUndone(this.card, this.kind, this.result); final FeedCard card; final SwipeKind kind; final SwipeResult result; }
final class BlockAccepted extends SwipeEvent { BlockAccepted(this.senderName); final String senderName; }

class ToastMessage { final String text; final bool showUndo; }

class SwipeController extends ChangeNotifier {
  SwipeController({required ApiClient api, required FeedModel feed, required IdGenerator ids,
                   DateTime Function()? now, FilingSheetLauncher? fileLauncher});
  bool get canUndo;
  bool get enabled;                 // false while offline or no card
  ToastMessage? get toast;
  bool holdPrompts;                 // T-1009 sets true during a Blitz round
  List<BlockPrompt> takeHeldPrompts();
  Stream<SwipeEvent> get events;    // T-1008a and T-1009 listen
  Future<void> keep();
  Future<void> skip();
  Future<void> reject();
  Future<void> file(BuildContext context);
  Future<void> undo();
}

const Duration kUnsubDelay = Duration(minutes: 5);   // UNSUB_DELAY [TUNABLE], S2 glossary
const int kPersonalBlockThreshold = 3;               // PERSONAL_BLOCK_THRESHOLD [TUNABLE], S2 glossary
```

## Algorithm

1. **A swipe** (`keep`, `skip`, `reject`, or `file` after a choice): if `!enabled` return. If the current item is a divider, any swipe calls `feed.dismissDivider()` and sends nothing. Otherwise `card = feed.takeCurrent()`, `key = ids.uuidV4()`, push `UndoEntry(card, kind, key, ack)` onto the undo stack, set the optimistic toast (step 4) and enqueue the send.
2. **Serial sending** (S7 5.5): keep one `Future` chain `_tail = _tail.then((_) => _send(entry))` so swipes and undos go out in order. `_send` calls `api.swipe(req, idempotencyKey: key)`:
   - `NetworkException`: retry with the same key after 1 s, 2 s, 4 s `[DEFAULT]`; after that treat as failure.
   - `ApiException` `message_changed`: remove the entry from the stack, hide the toast, card stays gone (FD-04 AC1).
   - Any other `ApiException` or exhausted retries: remove the entry, `feed.putBackOnTop(card)`, toast `Copy.actionFailed` without Undo (XC-04).
   - Success: store the result, emit `SwipeAcked`, update the toast text from the outcome if it is still showing this entry, handle `prompts` (step 6).
3. **File:** `choice = await fileLauncher(context, card)` before anything is sent; null means cancel, the card returns, nothing is sent. The default launcher in this task returns null; T-1003 supplies the real one.
4. **Toast text.** Optimistic, from the card: reject on `list` with `hasOneClick` uses `Copy.trashedUnsubscribing(kUnsubDelay)`; other rejects use `Copy.trashed`; keep `Copy.kept`; skip `Copy.skipped`; file `Copy.filed(name)`. On ack replace it with the outcome text:

| Outcome | Copy |
| --- | --- |
| `kept` | "Kept." (S9) |
| `skipped` `[DEFAULT]` | "Skipped. It'll come back later." |
| `trashed_unsubscribe_queued` | "Trashed. Unsubscribing in 5 minutes." (S9; minutes = ceil(`unsubscribe_due_at` minus now), at least 1, "1 minute" singular) |
| `trashed_unsubscribe_manual` | "Trashed. The unsubscribe link is in Needs Attention." (S9) |
| `trashed_list_no_unsubscribe` `[DEFAULT]` | "Trashed. Future mail from this sender will be trashed too." |
| `trashed` `[DEFAULT]` | "Trashed." |
| `reported_spam` `[DEFAULT]` | "Reported as spam and trashed." |
| `filed` `[DEFAULT]` | "Filed under <filed_category.name>." |

   The ack-time update matters because a mailto list has `has_one_click` false yet queues an unsubscribe. Show the toast as a SnackBar for 4 seconds `[DEFAULT]` with an "Undo" action.
5. **Undo** (button always present, disabled when the stack is empty): pop the last entry; enqueue on the same chain: await its ack; if it failed (card already back) stop. Else `api.undo(result.undoToken)`:
   - Success: `feed.putBackOnTop(card)`, emit `SwipeUndone`; toast `Copy.restoredAlreadySent` if `unsubscribeAlreadySent`, else `Copy.restoredSpam` if the outcome was `reported_spam`, else `Copy.restored`.
   - `410 undo_expired`: clear the stack, toast `Copy.cannotUndo` `[DEFAULT]`.
   - Other error: push the entry back, toast `Copy.actionFailed`.
6. **Block prompt** (PB-01): for each `block_person` prompt, if `holdPrompts` add to a held list; else show `AlertDialog` with `Copy.blockQuestion(name)`, buttons "Not now" and "Block" (Block has `autofocus: true` and is the filled button, so it is preselected). Block: `createBlockRule(promptRef)`, emit `BlockAccepted`, toast `Copy.blocked` `[DEFAULT]`. Not now: `declineBlockPrompt(promptRef)`. Errors show `Copy.actionFailed`.
7. **Gestures** (`SwipeableCard`): `GestureDetector` pan; the card follows the finger. On release the dominant axis wins: right keep, left reject, up file, down skip, when distance is at least 100 logical px or velocity at least 800 px/s `[DEFAULT]`; otherwise spring back. Fly-off animation 250 ms; when `MediaQuery.disableAnimationsOf(context)` is true, skip the animation.
8. **Buttons:** Reject, Skip, File, Keep, Undo; each an `IconButton` or `TextButton` with a Semantics label equal to its name; each calls the same controller method as its gesture (gesture parity, S10 3.2). Disabled while `!enabled` (offline included).

Copy constants not shown in the table: `trashedUnsubscribing(d)` (built from the S9 sentence), `restored` `[DEFAULT]` "Restored.", `restoredAlreadySent` "Restored. The unsubscribe request had already been sent." (S9), `restoredSpam` `[DEFAULT]` "Restored. The spam report itself can't be recalled.", `cannotUndo` `[DEFAULT]` "Can't undo that any more.", `blockQuestion(n)` "You've rejected $n 3 times. Block them?" (S9; 3 from `kPersonalBlockThreshold`), `block` "Block", `notNow` "Not now", `blocked` `[DEFAULT]` "Blocked.", button labels "Keep", "Reject", "File", "Skip", "Undo".

## Acceptance criteria

| ID | Behaviour (one line) |
| --- | --- |
| SW-01 AC1 | Right swipe and Keep send `keep` and show the next card |
| SW-02 AC1 | Down swipe and Skip send `skip` and show the next card |
| SW-03 AC1 | Left swipe and Reject send `reject` |
| SW-03 AC2 | The reject toast states the unsubscribe delay |
| SW-03 AC3 | A `reported_spam` outcome shows the spam toast |
| SW-04 AC1 | Up swipe and File both open the filing sheet launcher |
| SW-05 AC1 | Undo returns the card to the top after the server restores it |
| SW-05 AC3 | Undo after the unsubscribe went says it had already been sent |
| SW-05 AC4 | Undo works back through every swipe in order |
| SW-05 AC4a | Undoing a spam report says the report cannot be recalled |
| FD-04 AC1 | `409 message_changed` drops the card silently |
| PB-01 AC1 | The block prompt shows with Block preselected |
| PB-01 AC3 | Not now sends the decline |
| XC-03 | Every swipe has a labelled button; reduced motion skips the fly-off |
| XC-04 | A provider error returns the card with "Couldn't do that. Try again." |

## Tests that must pass

- `'SW-01 AC1 swipe right and Keep both send keep'` (widget, gesture parity)
- `'SW-02 AC1 swipe down and Skip both send skip'` (widget)
- `'SW-03 AC1 swipe left and Reject both send reject'` (widget)
- `'SW-04 AC1 swipe up and File both open the filing launcher'` (widget)
- `'SW-03 AC2 reject toast states the delay'` (widget)
- `'SW-03 AC2 toast follows the server outcome for a mailto unsubscribe'` (widget)
- `'SW-03 AC3 reported_spam toast'` (widget)
- `'SW-05 AC1 undo restores the card to the top'` (widget)
- `'SW-05 AC3 undo after the unsubscribe went says it had already been sent'` (widget)
- `'SW-05 AC4 undo works back through every swipe in order'` (unit)
- `'SW-05 AC4a undo of a spam report says the report cannot be recalled'` (widget)
- `'FD-04 AC1 message_changed drops the card silently'` (widget)
- `'PB-01 AC1 block prompt shows with Block preselected'` (widget)
- `'PB-01 AC3 Not now sends decline'` (widget)
- `'XC-03 every swipe has a labelled button'` (widget)
- `'XC-03 reduced motion skips the fly-off animation'` (widget, `MediaQueryData(disableAnimations: true)`, no running animations after one pump)
- `'XC-04 provider error returns the card with Couldn\'t do that'` (widget)
- `'s9_feed_action_failed card returns to the top'` (widget)
- `'s9_feed_message_changed next card shows'` (widget)
- `'s9_feed_undo_disabled before any swipe'` (widget)
- `'s9_feed_toast_manual_unsubscribe names Needs Attention'` (widget)
- `'s9_feed_offline disables swipe buttons'` (widget)
- `'swipes are sent one at a time in order'` (unit)
- `'undo waits for the swipe ack before sending'` (unit)
- `'network error retries with the same Idempotency-Key'` (unit)
- `'divider is dismissed by any swipe without an API call'` (widget)

## Edge cases and traps

- One `Idempotency-Key` per swipe, reused on every retry of that swipe; a new key for each new swipe.
- Never send undo before its swipe's ack (S7 5.5): put undo on the same chain.
- `message_changed` is not an error to show; any other `409` is.
- File with no choice sends nothing; a cancelled filing sheet must not be recorded as a skip.
- The toast copy for the delay comes from the server's `unsubscribe_due_at` after the ack, not a hard-coded "5".
- Keep the undo stack in memory only; no storage, and it is empty in a new app instance.
- Use `IdGenerator` and injected `now` in tests; no `Random()` or `DateTime.now()` in the controller.
- Do not open the block dialog during a Blitz round when `holdPrompts` is true.

## Out of scope

- The filing sheet (T-1003); celebrations, meter and round totals (T-1008a); animations beyond the fly-off, sounds and combo (T-1008b); Blitz (T-1009).

## Done when

- The tests above pass and every required check is green (S10 10.1).
- Definition of done in S10 10.4.
