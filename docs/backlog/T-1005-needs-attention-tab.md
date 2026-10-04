# T-1005: Needs Attention tab

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M10 | sonnet | about 300 lines of code plus tests | T-007, T-1001a |

**Read only these spec sections:** S9 section 6 and section 9 "Copy for API outcomes" (`docs/specs/S9-functional-screens.md`); S7 section 5.8 (API-NA-1 to API-NA-3) and the `NeedsAttentionItem` schema in `docs/specs/S7-api-contract.openapi.yaml`; S2 NA-01, UN-01 AC6, UN-04 AC6, UN-05 AC1, XC-04. Nothing else is needed.

## Goal

The Needs Attention tab lists open items newest first, each with sender, mailbox badge, the reason in plain words and when it was raised, and the actions "Open unsubscribe page", "Done" and "Dismiss". The tab shows a badge with the open item count.

## Files

| Action | Path | What |
| --- | --- | --- |
| Create | `app/lib/api/models/needs_attention.dart` | `NeedsAttentionItem`, `NeedsAttentionPage`, `NaReason` |
| Create | `app/lib/state/needs_attention_model.dart` | `NeedsAttentionModel` (provided at app root) |
| Create | `app/lib/screens/needs_attention/needs_attention_screen.dart` | The tab |
| Change | T-007's shell (bottom tabs) | `Badge` with `openCount` on the Needs Attention tab |
| Change | `app/lib/api/api_client.dart`, `http_api_client.dart`, `fake_api_client.dart` | Methods below |
| Change | `app/lib/copy.dart` | Strings below |
| Create | `app/test/screens/needs_attention_test.dart` | Tests below |

## Types and signatures

```dart
enum NaLoadState { loading, ready, failed }

enum NaReason {
  httpsOnlyUnsubscribe, oneClickRedirect, oneClickAddressRefused, unsubscribeFailed,
  unsubscribeIgnored, jobExpired, mailboxNeedsSignIn, other,
}
NaReason parseNaReason(String raw); // explicit switch; v2 codes (captcha, login_required, page_*) and unknown -> other

class NeedsAttentionItem { final String itemId, mailboxId, senderDisplay; final NaReason reason; final Uri? link; final DateTime createdAt; }
class NeedsAttentionPage { final List<NeedsAttentionItem> items; final int openCount; final String? nextCursor; }

abstract class ApiClient {
  Future<NeedsAttentionPage> listNeedsAttention({String? cursor, int limit = 20}); // GET /needs-attention
  Future<void> resolveNeedsAttention(String itemId);                               // POST .../resolve
  Future<void> dismissNeedsAttention(String itemId);                               // POST .../dismiss
}

class NeedsAttentionModel extends ChangeNotifier {
  NeedsAttentionModel({required ApiClient api, Duration refreshEvery = const Duration(minutes: 5)});
  NaLoadState get state; List<NeedsAttentionItem> get items; int get openCount;
  Future<void> load();
  Future<void> resolve(NeedsAttentionItem i);
  Future<void> dismiss(NeedsAttentionItem i);
}
```

## Algorithm

1. Load on app open (after sign-in), when the tab is selected, and every 5 minutes while the app is visible `[DEFAULT]`. `openCount` drives the tab badge (hidden at 0).
2. Sort items by `createdAt` descending on the client as well (NA-01 AC1).
3. Each row: `senderDisplay`, mailbox badge (address from `session.mailboxes`), reason copy (table below), `formatDateTime(createdAt)`, then the actions.
4. "Open unsubscribe page": shown when `link != null` and the reason allows it (table). Pass `link` through `safeNavigationTarget(..., allowLoopbackHttp: false)` and `browser.openExternal`. The link is only ever the item's `link` field from the server (UN-04 AC6); never build one from any other text.
5. "Sign in again" for `mailboxNeedsSignIn`: `startAuth(intent: reconnect, mailboxId: item.mailboxId)` then `browser.assign`.
6. "Done" calls `resolveNeedsAttention`, "Dismiss" calls `dismissNeedsAttention`. Remove the row at once; on error put it back and show `Copy.actionFailed` (XC-04). Decrease `openCount` on success.
7. States: `loading` spinner; empty `Copy.nothingNeedsYou`; load failure `Copy.actionFailed` with "Try again".

Reason copy (S9 verbatim unless marked):

| Reason | Text | Open button |
| --- | --- | --- |
| `httpsOnlyUnsubscribe` | This sender needs you to unsubscribe on their website. | Yes |
| `oneClickRedirect` | The unsubscribe request was redirected, so we stopped. Open the page to finish. | Yes |
| `oneClickAddressRefused` | We couldn't safely send this unsubscribe request. Check the sender's own unsubscribe options. | No |
| `unsubscribeIgnored` | Mail still arriving after unsubscribe | Yes |
| `unsubscribeFailed` `[DEFAULT]` | We couldn't unsubscribe you from this sender. Open the page to finish. | Yes |
| `jobExpired` `[DEFAULT]` | The unsubscribe request didn't finish in time. Open the page to finish. | Yes |
| `mailboxNeedsSignIn` | Couldn't unsubscribe from <sender>. Sign in again to <address>. | "Sign in again" instead |
| `other` `[DEFAULT]` | This needs your attention. | Yes when a link exists |

`mailboxNeedsSignIn` `[DEFAULT]`: S7 has no reason code for UN-01 AC6. Treat the wire value `sign_in_required` as this reason, and also treat `unsubscribe_failed` as this reason when the item's mailbox has status `needs_sign_in` in the session. The PR notes this pending a spec fix.

Other copy: `openUnsubscribePage` "Open unsubscribe page"; `done` "Done"; `dismiss` "Dismiss"; `nothingNeedsYou` "Nothing needs you."

## Acceptance criteria

| ID | Behaviour (one line) |
| --- | --- |
| NA-01 AC1 | Every open item is listed newest first with sender, reason and the three actions |
| NA-01 AC2 | Done and Dismiss remove the item (client calls resolve and dismiss) |
| UN-01 AC6 | An item for a signed-out mailbox says "Sign in again" with that action |
| UN-04 AC6 | "Open unsubscribe page" opens only the item's server-provided https link |
| UN-05 AC1 | A failed job's item shows the sender, the reason and an open action |
| XC-04 | A failed Done or Dismiss puts the item back with a visible message |

## Tests that must pass

- `'NA-01 AC1 lists items newest first with sender, mailbox badge, reason and actions'` (widget)
- `'NA-01 AC2 Done resolves and Dismiss dismisses'` (widget)
- `'UN-01 AC6 signed-out mailbox item offers Sign in again'` (widget)
- `'UN-04 AC6 Open unsubscribe page opens the item link only'` (widget)
- `'UN-05 AC1 failed job item offers Open unsubscribe page'` (widget)
- `'XC-04 failed Done puts the item back with Couldn\'t do that'` (widget)
- `'s9_needs_attention_empty shows Nothing needs you'` (widget)
- `'s9_needs_attention_loading shows a progress indicator'` (widget)
- `'s9_needs_attention_badge shows open_count'` (widget)
- `'s9_needs_attention_reason_copy matches S9 for each v1 reason'` (widget, one case per reason)
- `'a javascript or http link is never opened'` (widget)
- `'XC-03 Needs Attention controls are labelled'` (widget)

## Edge cases and traps

- Never open `javascript:`, `data:` or plain `http:` links even if the server sent one; the server drops them, the app checks again.
- Never parse URLs out of `senderDisplay` or any other text.
- `oneClickAddressRefused` has no open button: the target was a private address.
- Badge count comes from `open_count`, not `items.length` (paging).
- Sender display is plain `Text`.

## Out of scope

- Creating items (T-605, T-702 to T-707); the badge's visual design (later).

## Done when

- The tests above pass and every required check is green (S10 10.1).
- Definition of done in S10 10.4.
