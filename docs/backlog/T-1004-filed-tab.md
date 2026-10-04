# T-1004: Filed tab

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M10 | sonnet | about 330 lines of code plus tests | T-007, T-1001a |

**Read only these spec sections:** S9 section 5 and section 9 (`docs/specs/S9-functional-screens.md`); S7 section 5.6 (API-CAT-1 to API-CAT-5) and the `Category`, `FiledMessage`, `MailboxError` schemas in `docs/specs/S7-api-contract.openapi.yaml`; S2 FL-05. Nothing else is needed.

## Goal

The Filed tab lists the user's categories with message counts across all mailboxes; tapping one lists its messages (sender, subject, date, mailbox badge), and tapping a message opens it in Gmail on the web. Categories can be renamed and deleted (label only, never messages). This task also owns the `Category` model and `listCategories`, which T-1003 and T-1006b reuse.

## Files

| Action | Path | What |
| --- | --- | --- |
| Create | `app/lib/api/models/category.dart` | `Category`, `CategoryMailboxCount`, `FiledMessage`, `FiledMessagePage` |
| Create | `app/lib/state/filed_model.dart` | `FiledModel`, `CategoryMessagesModel` |
| Create | `app/lib/screens/filed/filed_screen.dart` | Category list with rename and delete |
| Create | `app/lib/screens/filed/category_messages_screen.dart` | Messages in one category |
| Change | `app/lib/api/api_client.dart`, `http_api_client.dart`, `fake_api_client.dart` | Methods below |
| Change | `app/lib/routes.dart` | Route `/filed/category` (argument `Category`) |
| Change | `app/lib/copy.dart` | Strings below |
| Create | `app/test/screens/filed_test.dart` | Tests below |

## Types and signatures

```dart
class Category { final String categoryId; final String name; final int messageCount; final List<CategoryMailboxCount> perMailbox; }
class CategoryMailboxCount { final String mailboxId; final int messageCount; }
class FiledMessage { final String mailboxId, messageId, senderName, subject; final DateTime receivedAt; final Uri providerWebUrl; }
class FiledMessagePage { final List<FiledMessage> messages; final String? nextCursor; final List<MailboxError> mailboxErrors; }

abstract class ApiClient {
  Future<List<Category>> listCategories();                                   // GET /categories
  Future<Category> renameCategory(String categoryId, String name);            // PATCH /categories/{id}
  Future<void> deleteCategory(String categoryId);                             // DELETE /categories/{id}
  Future<FiledMessagePage> listCategoryMessages(String categoryId, {String? cursor, int limit = 20}); // GET /categories/{id}/messages
}

enum LoadState { loading, ready, failed }
class FiledModel extends ChangeNotifier {
  FiledModel({required ApiClient api});
  LoadState get state; List<Category> get categories;
  Future<void> load();
  Future<String?> rename(Category c, String newName);   // returns error copy or null
  Future<String?> delete(Category c);
}
class CategoryMessagesModel extends ChangeNotifier {
  CategoryMessagesModel({required ApiClient api, required Category category});
  LoadState get state; List<FiledMessage> get messages; List<MailboxError> get mailboxErrors;
  Future<void> loadFirst(); Future<void> loadMore();
}
```

## Algorithm

1. Filed tab opens: `FiledModel.load()`; `loading` shows a spinner; empty list shows `Copy.filedEmpty`; failure shows `Copy.actionFailed` with "Try again".
2. Each row: category name and `formatCount(messageCount)` (T-1001a). A trailing menu (Semantics "More for <name>") has "Rename" and "Delete".
3. Rename: dialog with a text field prefilled with the name. Validate 1 to 100 characters after trimming, not starting or ending with `/` (S7 5.5 rule, Gmail nesting); invalid shows `Copy.categoryNameRule`. `409 category_exists` shows `Copy.categoryExists(name)`. Success replaces the row.
4. Delete: confirm dialog `Copy.deleteCategoryQuestion(name)` with "Cancel" and "Delete". Delete calls `deleteCategory`; success removes the row.
5. Tap a row: push `/filed/category`. `CategoryMessagesModel.loadFirst()`; rows show sender name, subject (one line), `formatDate(receivedAt)`, and the mailbox badge (address from `session.mailboxes`). Load more when scrolled within 3 rows of the end and `nextCursor != null`.
6. Per-mailbox provider error (S9 state): for each `mailboxErrors` entry show a banner `Copy.filedMailboxUnavailable(address)` `[DEFAULT]` above the list; other mailboxes' messages still show.
7. Tap a message: `safeNavigationTarget(providerWebUrl.toString(), allowLoopbackHttp: kE2eBuild)`; if non-null `browser.openExternal(url)`; otherwise do nothing.

Copy (S9 verbatim unless marked): `filedEmpty` "Swipe up on an email to start filing."; `rename` `[DEFAULT]` "Rename"; `delete` `[DEFAULT]` "Delete"; `categoryNameRule` `[DEFAULT]` "Use 1 to 100 characters, not starting or ending with /."; `categoryExists(n)` `[DEFAULT]` "You already have a category called $n."; `deleteCategoryQuestion(n)` `[DEFAULT]` "Delete $n? The label is removed from your mailboxes. Your messages stay where they are."; `filedMailboxUnavailable(a)` `[DEFAULT]` "Can't reach $a right now. Some messages may be missing."

## Acceptance criteria

| ID | Behaviour (one line) |
| --- | --- |
| FL-05 AC1 | The Filed tab lists categories with message counts and opens a category to show its messages |

## Tests that must pass

- `'FL-05 AC1 Filed lists categories with counts'` (widget)
- `'FL-05 AC1 tapping a category shows its messages with sender, subject, date and mailbox'` (widget)
- `'s9_filed_empty shows the start filing message'` (widget)
- `'s9_filed_loading shows a progress indicator'` (widget)
- `'s9_filed_provider_error_per_mailbox shows a banner and other messages'` (widget)
- `'rename sends PATCH and shows the clash message on category_exists'` (widget)
- `'rename refuses a name starting with a slash'` (widget)
- `'delete asks for confirmation and sends DELETE'` (widget)
- `'tapping a message opens provider_web_url in a new tab'` (widget, `FakeBrowser.openExternal`)
- `'a non-https provider url is not opened'` (widget)
- `'XC-03 Filed controls are labelled'` (widget)

## Edge cases and traps

- Delete removes the label only; never offer to delete messages, and the copy must say messages stay (INV-5).
- Category names and subjects are plain `Text`.
- Open messages with `openExternal` (noopener, noreferrer), never in the app's own tab.
- Compare names case-insensitively when checking clashes locally; the server is the authority.
- No caching of categories or messages in browser storage.

## Out of scope

- Filing a card (T-1003); filing rules (T-1006b).

## Done when

- The tests above pass and every required check is green (S10 10.1).
- Definition of done in S10 10.4.
