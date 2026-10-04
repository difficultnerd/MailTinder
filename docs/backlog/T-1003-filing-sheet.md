# T-1003: Filing sheet and keep-learning prompt

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M10 | sonnet | about 320 lines of code plus tests | T-1002b, T-1004 |

**Read only these spec sections:** S9 section 4 and the "Keep-learning prompt" bullet of section 3 Overlays (`docs/specs/S9-functional-screens.md`); S7 section 5.4 (`suggestion`, `keep_prompt`), 5.5 (`file` rules) and API-RULE-2 `file` form in 5.7 (`docs/specs/S7-api-contract.md`); S2 SW-04, FL-01 AC3, FL-02, FL-03, FL-04. Nothing else is needed.

## Goal

An up-swipe or the File button opens a filing sheet built from the suggestion shipped on the card, with no network wait: the suggested category first, up to two alternates and "New category", or a single "File under <category>" once a sender is learned. The keep-learning prompt appears inline on cards that carry `keep_prompt`.

## Files

| Action | Path | What |
| --- | --- | --- |
| Create | `app/lib/screens/feed/filing_sheet.dart` | `FilingSheet` widget and `showFilingSheet` (a `FilingSheetLauncher`) |
| Create | `app/lib/state/categories_cache.dart` | `CategoriesCache` (in memory, loaded once per session) |
| Create | `app/lib/screens/feed/keep_prompt.dart` | Inline prompt |
| Change | `app/lib/screens/feed/card_view.dart` | Show `KeepPrompt` when `card.keepPrompt != null` |
| Change | `app/lib/screens/feed/feed_screen.dart` | Pass `showFilingSheet` as the controller's `fileLauncher` |
| Change | `app/lib/api/api_client.dart`, `http_api_client.dart`, `fake_api_client.dart` | `createFileRule` |
| Change | `app/lib/state/swipe_controller.dart` | `fileWith(FeedCard, FilingChoice)` used by the keep prompt; `409 category_exists` retry |
| Change | `app/lib/copy.dart` | Strings below |
| Create | `app/test/screens/filing_sheet_test.dart` | Tests below |

## Types and signatures

```dart
abstract class ApiClient {
  /// POST /rules {kind: "file", mailbox_id, message_id, category_id}
  Future<void> createFileRule({required String mailboxId, required String messageId, required String categoryId});
}

class CategoriesCache extends ChangeNotifier {
  CategoriesCache({required ApiClient api});
  List<Category>? get categories;          // null until loaded
  Future<void> ensureLoaded();             // one listCategories call per session; errors leave null
  Category? findByName(String name);       // case-insensitive, trimmed
}

/// NameProposer is the hook for an on-device model (FL-02 AC2). v1 returns null.
typedef NameProposer = Future<String?> Function(FeedCard card);

Future<FilingChoice?> showFilingSheet(BuildContext context, FeedCard card,
    {required CategoriesCache cache, NameProposer? proposer, Duration suggestionBudget = const Duration(milliseconds: 200)});

class KeepPrompt extends StatefulWidget {
  const KeepPrompt({super.key, required this.card, required this.onFile});
  final FeedCard card;
  final Future<void> Function() onFile;
}
```

## Algorithm

1. Start `cache.ensureLoaded()` when the Feed opens (background), so names are known before the first up-swipe.
2. `showFilingSheet` opens a modal bottom sheet in the same frame as the swipe; the first frame must already show the choices from `card.suggestion` (FL-01 AC3: the suggestion ships with the card, no round trip).
3. Choices by `suggestion.confidence`:
   - `learned`: one big button `Copy.fileUnder(name)` and an "Other" button that expands the alternates and "New category" (FL-03 AC1).
   - `suggested`: the suggestion first, then up to two alternates, then "New category" (SW-04 AC1).
   - `none`, or `suggestion == null` with no cached categories: go straight to the name field (SW-04 AC3, S9 "no categories yet").
   - `suggestion == null` with cached categories: show up to three cached categories by highest `messageCount` and "New category".
   - "Loading suggestion" state: only when `suggestion == null` and the cache is still loading; show a spinner for at most `suggestionBudget`, then show "New category" only (S9 section 4).
4. Tapping a category returns `FilingChoice(categoryId: id)`. Filing never happens without a tap (FL-03 AC2): no auto-select, no timer that files.
5. "New category": a text field (prefilled with `proposer(card)` when it returns a value; v1 passes no proposer). Validate 1 to 100 characters trimmed, no leading or trailing `/`, error `Copy.categoryNameRule` (T-1004). On submit, if `cache.findByName(name)` matches, return its `categoryId` (S9 "name already exists (selects the existing one)"); else return `FilingChoice(newCategoryName: name)`.
6. Cancel ("Cancel" button, drag down, or barrier tap) returns null; T-1002b puts the card back and sends nothing.
7. In `SwipeController`: if a `file` swipe with `newCategoryName` gets `409 category_exists`, reload the cache, find the name, and resend once with `categoryId` and a new idempotency key.
8. Keep-learning prompt (FL-04): inline, quiet text `Copy.keepPrompt(categoryName)` with a "File" button and a dismiss icon (Semantics "Dismiss"). File: `createFileRule(mailboxId, messageId, categoryId)`, then `swipeController.fileWith(card, FilingChoice(categoryId: id))` so this card is filed too `[DEFAULT]`; errors show `Copy.actionFailed` and nothing is filed. Dismiss hides it for this card only and sends nothing (FL-04 AC2 "ignoring it has no effect").

Copy (S9 verbatim unless marked): `newCategory` "New category"; `fileUnder(n)` "File under $n"; `other` "Other"; `categoryNameHint` `[DEFAULT]` "Category name"; `fileButton` "File"; `keepPrompt(n)` "You always keep these. File under $n?"; `dismiss` `[DEFAULT]` "Dismiss".

## Acceptance criteria

| ID | Behaviour (one line) |
| --- | --- |
| SW-04 AC1 | The sheet shows the suggestion first, up to two alternates and "New category" |
| SW-04 AC3 | With no suggestion the sheet asks for a new category name |
| FL-01 AC3 | The sheet's choices are on screen in the first frame with no API call (client half of the 200 ms target) |
| FL-02 AC1 | A new name is sent as `new_category_name` |
| FL-03 AC1 | A learned sender shows one-tap "File under <category>" with alternates collapsed |
| FL-03 AC2 | Nothing is filed without a tap |
| FL-04 AC1 | A card with `keep_prompt` shows the quiet prompt |
| FL-04 AC2 | Accepting creates a filing rule; dismissing sends nothing |

## Tests that must pass

- `'SW-04 AC1 sheet shows suggestion first, two alternates and New category'` (widget)
- `'SW-04 AC3 no suggestion opens the name field'` (widget)
- `'FL-01 AC3 sheet choices render in the first frame without an API call'` (widget, `FakeApiClient.calls` unchanged after one pump)
- `'FL-02 AC1 a new category name is sent as new_category_name'` (widget)
- `'FL-03 AC1 learned sender shows File under with Other collapsed'` (widget)
- `'FL-03 AC2 nothing is filed without a tap'` (widget, pump 10 seconds, no swipe call)
- `'FL-04 AC1 keep prompt shows on a card with keep_prompt'` (widget)
- `'FL-04 AC2 accepting the keep prompt creates a file rule'` (widget)
- `'FL-04 AC2 dismissing the keep prompt sends nothing'` (widget)
- `'s9_filing_sheet_loading_suggestion falls back after 200 ms'` (widget)
- `'s9_filing_sheet_no_categories goes straight to the name field'` (widget)
- `'s9_filing_sheet_name_exists selects the existing category'` (widget)
- `'s9_filing_sheet_cancel returns the card and sends nothing'` (widget)
- `'category_exists on a new name resends once with the category id'` (unit)
- `'XC-03 filing sheet controls are labelled'` (widget)

## Edge cases and traps

- Do not call `listCategories` when the sheet opens; it must render from the card and the cache already in memory.
- A cancelled sheet is not a skip; send nothing.
- The keep prompt is quiet: no dialog, no sound, no auto-file.
- FL-02 AC2 (on-device name proposal) is a hook only; do not add a model or package.
- Cache lives in memory; never browser storage.

## Out of scope

- Gemini Nano name proposals: T-1010.
- Filing rules list and switches (T-1006b).

## Done when

- The tests above pass and every required check is green (S10 10.1).
- Definition of done in S10 10.4.
