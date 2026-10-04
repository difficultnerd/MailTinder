# T-1002a: Feed screen, cards and Feed states

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M10 | sonnet | about 420 lines of code plus tests | T-007, T-1001a |

**Read only these spec sections:** S9 section 3 (card content, bulk badge tap, States list) and section 9 (`docs/specs/S9-functional-screens.md`); S7 section 5.4 (API-FEED-1 and the `Card` table) and the `Card`, `FeedPage`, `MailboxError`, `Suggestion`, `CategoryRef` schemas in `docs/specs/S7-api-contract.openapi.yaml`; S2 FD-01 to FD-04 and XC-03. Nothing else is needed.

## Goal

The Feed tab loads cards from API-FEED-1 and shows one card in focus with the next card partly visible behind it, plus every non-swipe Feed state in S9: loading, normal, the up-to-date divider, empty, one mailbox failing, all mailboxes needing sign-in, offline and load failed. Swipes come in T-1002b.

## Files

| Action | Path | What |
| --- | --- | --- |
| Create | `app/lib/api/models/feed.dart` | `FeedCard`, `Suggestion`, `CategoryRef`, `BossInfo`, `MailboxError`, `FeedPage` with `fromJson` |
| Create | `app/lib/state/feed_model.dart` | `FeedModel`, `FeedItem`, `CardItem`, `DividerItem` |
| Create | `app/lib/platform/connectivity.dart`, `connectivity_web.dart`, `connectivity_stub.dart` | `Connectivity` (online flag and changes) |
| Create | `app/lib/screens/feed/feed_screen.dart` | Screen, banners, pull to refresh |
| Create | `app/lib/screens/feed/card_view.dart` | One card |
| Create | `app/lib/screens/feed/bulk_badge.dart` | Badge and reason sheet |
| Create | `app/lib/screens/feed/divider_card.dart` | Up-to-date divider |
| Change | `app/lib/api/api_client.dart`, `http_api_client.dart`, `fake_api_client.dart` | `feedNext` |
| Change | `app/lib/copy.dart` | Strings below |
| Create | `app/test/support/fake_connectivity.dart`, `app/test/support/cards.dart` | Fakes and card builders (synthetic data, example.com only) |
| Create | `app/test/screens/feed_states_test.dart`, `app/test/unit/feed_model_test.dart` | Tests below |

## Types and signatures

```dart
// Named FeedCard because Flutter already has a Material `Card` widget.
class FeedCard {
  final String mailboxId, messageId, senderName, senderAddress, subject, preview, bulkReason;
  final DateTime receivedAt;
  final int bulkScore, skipCount;
  final MessageClass messageClass;        // wire field "class"
  final bool hasOneClick;
  final Suggestion? suggestion;
  final CategoryRef? keepPrompt;
  final BossInfo? boss;
  final Uri providerWebUrl;
  final String classificationToken;
  final String? classifierId;             // admins only
  factory FeedCard.fromJson(Map<String, dynamic> j);
}
enum MessageClass { list, bulkNoHeader, notice, personal, suspect }
class Suggestion { final String? categoryId; final String? name; final List<CategoryRef> alternates; final SuggestionConfidence confidence; }
enum SuggestionConfidence { learned, suggested, none }
class CategoryRef { final String categoryId; final String name; }
class BossInfo { final int remaining; }
class MailboxError { final String mailboxId; final String code; }
class FeedPage { final List<FeedCard> cards; final String? nextCursor; final String phase; final bool phaseChanged;
                 final List<MailboxError> mailboxErrors; final int ruleActionsApplied; }

abstract class ApiClient {
  /// POST /api/v1/feed/next {cursor, limit, refresh}
  Future<FeedPage> feedNext({String? cursor, int limit = 20, bool refresh = false});
}

abstract class Connectivity { bool get isOnline; Stream<bool> get changes; }

sealed class FeedItem {}
final class CardItem extends FeedItem { CardItem(this.card); final FeedCard card; }
final class DividerItem extends FeedItem {}

enum FeedStatus { loading, ready, empty, allNeedSignIn, loadFailed }

class FeedModel extends ChangeNotifier {
  FeedModel({required ApiClient api, required SessionModel session, required Connectivity connectivity,
             int pageSize = 20, int refillBelow = 5});
  FeedStatus get status;
  FeedItem? get current;
  FeedItem? get next;
  List<MailboxError> get mailboxErrors;
  bool get offline;
  Future<void> open();                 // cursor null, refresh true
  Future<void> refresh();              // pull to refresh: clears the queue, same call as open
  Future<void> loadMoreIfNeeded();     // when fewer than refillBelow items remain and nextCursor != null
  void dismissDivider();
  // Used by T-1002b:
  FeedCard? takeCurrent();             // removes and returns the current card (null for a divider)
  void putBackOnTop(FeedCard card);
  void dropMessage(String mailboxId, String messageId);
}
```

## Algorithm

1. `open()` (on first build of the Feed tab, FD-03 AC5): status `loading`, call `feedNext(cursor: null, limit: 20, refresh: true)`.
2. Apply a page: if `phaseChanged` and the divider has not been shown this session, append a `DividerItem` before the page's cards (FD-03 AC2, once per `FeedModel`). Append the cards in the order received (server orders newest first, FD-02 AC1). Store `nextCursor` and replace `mailboxErrors`.
3. Status after a page: queue not empty: `ready`. Queue empty and `nextCursor != null`: fetch the next page at once, at most 3 empty pages in a row `[DEFAULT]` (rules may trash a whole page), then `empty`. Queue empty, no cursor: if every mailbox in `session.mailboxes` appears in `mailboxErrors` with code `sign_in_required`, `allNeedSignIn`; otherwise `empty`.
4. After any change to the queue call `loadMoreIfNeeded()`: if fewer than 5 items remain `[DEFAULT]` and a cursor exists and no load is in flight, fetch `feedNext(cursor: nextCursor, refresh: false)`.
5. Errors: `NetworkException` sets `offline` true; `ApiException` sets `loadFailed` only when the queue is empty (otherwise keep showing cards). `Connectivity.changes` true clears `offline` and, if the queue is empty, calls `open()` again.
6. `refresh()` (pull to refresh, `RefreshIndicator`): clear the queue and call `open()`'s request.
7. Card view (FD-01 AC1): Column of `Text` widgets: sender name, sender address, subject (max 2 lines), preview (max 6 lines), mailbox badge (the mailbox's `emailAddress` from `session.mailboxes`, matched by `mailboxId`; empty if missing), bulk badge, and `classifierId` in small text when present. All mail strings use plain `Text`, never `SelectableText.rich`, `Html` or Markdown (ASVS V1.1.2, V3.2.2). The next card sits behind, offset 12 px down and scaled 0.95 `[DEFAULT]`, excluded from semantics.
8. Bulk badge: text `Copy.bulkBadge(score)` `[DEFAULT]`; tap opens a bottom sheet with `bulkReason` as `Text` (S9 "Bulk badge tap").
9. Divider card: `Copy.upToDate` and a "Continue" button that calls `dismissDivider()`.
10. Banners above the card, one per failing mailbox (FD-02 AC3): code `sign_in_required` shows `Copy.mailboxNeedsSignIn(address)` with a "Sign in again" action that calls `startAuth(intent: reconnect, mailboxId: id)` and `browser.assign`; other codes show `Copy.mailboxUnavailable(address)` `[DEFAULT]`, no action.
11. Full-screen states: `loading` spinner with Semantics label `Copy.loadingCards`; `empty` shows `Copy.nothingToTriage`; `allNeedSignIn` shows `Copy.allNeedSignIn` and one "Sign in again to <address>" button per mailbox; `loadFailed` shows `Copy.actionFailed` and "Try again" (calls `open()`).
12. Offline banner `Copy.offline` `[DEFAULT]` while `offline` is true. T-1002b disables swipes from the same flag.
13. Connectivity web implementation: `window.navigator.onLine` plus `online` and `offline` events.

Copy (S9 verbatim unless marked):

| Constant | Text |
| --- | --- |
| `upToDate` | You're up to date. Now working back through older mail. |
| `continueLabel` `[DEFAULT]` | Continue |
| `nothingToTriage` | Nothing to triage. |
| `mailboxNeedsSignIn(a)` | Can't reach $a. Sign in again |
| `signInAgain` | Sign in again |
| `mailboxUnavailable(a)` `[DEFAULT]` | Can't reach $a right now. Pull down to try again. |
| `allNeedSignIn` `[DEFAULT]` | Sign in again to see your mail. |
| `signInAgainTo(a)` `[DEFAULT]` | Sign in again to $a |
| `offline` `[DEFAULT]` | You're offline. Swipes are paused until you're back online. |
| `loadingCards` `[DEFAULT]` | Loading cards |
| `bulkBadge(s)` `[DEFAULT]` | Bulk $s (Semantics: "Bulk score $s out of 100. Tap for the reason.") |

## Acceptance criteria

| ID | Behaviour (one line) |
| --- | --- |
| FD-01 AC1 | Exactly one card is in focus with sender name and address, subject, preview, mailbox badge and bulk badge |
| FD-01 AC2 | The preview renders as plain text (markup shows literally) |
| FD-02 AC2 | Each card shows its mailbox's address |
| FD-02 AC3 | One failing mailbox shows a banner naming it while other cards load |
| FD-03 AC2 | `phase_changed` shows the up-to-date divider once |
| FD-03 AC5 | Opening the Feed and pull to refresh both send `refresh: true` |
| ASVS V1.1.2 | Mail fields are output-encoded by Flutter `Text` |
| ASVS V3.2.2 | Hostile mail strings render as text, never as HTML |
| XC-03 | Feed controls are labelled and meet tap target guidelines |

## Tests that must pass

- `'FD-01 AC1 one card in focus shows every card field'` (widget)
- `'FD-01 AC2 preview renders as plain text'` (widget, preview `<b>x</b> example.com` found literally)
- `'FD-02 AC2 each card shows its mailbox address'` (widget)
- `'FD-02 AC3 one failing mailbox shows a banner naming it and other cards still load'` (widget)
- `'FD-03 AC2 phase_changed shows the up to date divider once'` (widget)
- `'FD-03 AC5 opening the Feed and pulling to refresh send refresh true'` (widget)
- `'asvs_v1_1_2 mail fields are rendered through Text widgets'` (widget)
- `'asvs_v3_2_2 hostile subject and sender name render as literal text'` (widget, script tag, RTL override, 998-character subject)
- `'XC-03 feed controls are labelled'` (widget)
- `'s9_feed_loading shows a progress indicator'` (widget)
- `'s9_feed_normal shows the next card behind'` (widget)
- `'s9_feed_empty shows Nothing to triage'` (widget)
- `'s9_feed_all_mailboxes_need_sign_in shows the full-screen prompt'` (widget)
- `'s9_feed_offline shows the offline banner'` (widget)
- `'s9_feed_load_failed shows Try again'` (widget)
- `'s9_feed_bulk_badge_tap shows the reason'` (widget)
- `'reconnect banner action starts reconnect for that mailbox'` (widget)
- `'feed loads the next page when fewer than five cards remain'` (unit)
- `'feed stops after three empty pages'` (unit)
- `'FeedCard.fromJson reads every S7 field and ignores unknown fields'` (unit)

## Edge cases and traps

- Never write cards, previews or cursors to browser storage; the queue lives in `FeedModel` only.
- `class` is a Dart keyword: map the JSON key `class` to `messageClass`; parse with an explicit `switch`, unknown values throw a `FormatException` (do not default to `personal`).
- `keep_prompt` and `boss` may be absent or `null`; `classifier_id` is absent for non-admins, not null.
- Do not re-order cards on the client; FD-02 AC1 ordering is the server's.
- Only one `feedNext` request in flight at a time.
- Do not show the full-screen sign-in prompt when cards exist; one failing mailbox is a banner only.
- Exclude the background card from semantics (`ExcludeSemantics`) so screen readers hear one card.
- Test data uses `example.com` addresses only.

## Out of scope

- Swipes, buttons, toasts, undo and the block prompt (T-1002b).
- Filing sheet and keep-learning prompt (T-1003); meter, level and boss banners (T-1008a).

## Done when

- The tests above pass and every required check is green (S10 10.1).
- Definition of done in S10 10.4.
