# T-1001a: Sign-in, invite link and Request invite screens

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M10 | sonnet | about 380 lines of code plus tests | T-007 |

**Read only these spec sections:** S9 section 1 (not 1.1), section 2, section 9 "Copy rules" and "Copy for API outcomes" (`docs/specs/S9-functional-screens.md`); S7 sections 3.2, 3.4 (intent table and outcome table), 5.2 (API-AUTH-1, API-AUTH-3, API-AUTH-4) and the API-INV-1 row of 5.1 (`docs/specs/S7-api-contract.md`); S2 AU-02, AU-03 and AU-07 AC1 (`docs/specs/S2-v1-acceptance-criteria.md`). Nothing else is needed.

## Goal

A person can open the app (or an invite link), tap "Continue with Google", come back from Google and land on the right screen: the Feed, Request invite, or a sign-in error with the S9 copy. This task also adds two small shared helpers every later screen uses: `Browser` (page navigation and new tabs, without browser storage) and `lib/format.dart` (dates and counts).

## Files

| Action | Path | What |
| --- | --- | --- |
| Create | `app/lib/platform/browser.dart` | `Browser` interface, `BrowserPopup`, `safeNavigationTarget`, conditional export of the implementation |
| Create | `app/lib/platform/browser_web.dart` | Web implementation using `package:web` |
| Create | `app/lib/platform/browser_stub.dart` | Non-web implementation that throws `UnsupportedError` (keeps `flutter test` on the VM compiling) |
| Create | `app/lib/format.dart` | `formatDate`, `formatDateTime`, `formatCount` |
| Create | `app/lib/api/models/auth.dart` | `AuthIntent`, `AuthOutcome`, `parseAuthOutcome` |
| Create | `app/lib/state/sign_in_model.dart` | `SignInModel` |
| Create | `app/lib/screens/sign_in/sign_in_screen.dart` | Sign-in screen, all S9 section 1 states except v2 |
| Create | `app/lib/screens/sign_in/auth_result_screen.dart` | Handles `/#/auth/result?outcome=...` |
| Create | `app/lib/screens/request_invite/request_invite_screen.dart` | S9 section 2 |
| Change | `app/lib/api/api_client.dart` | Add `startAuth`, `createInviteRequest` |
| Change | `app/lib/api/http_api_client.dart` | Implement them |
| Change | `app/lib/api/fake_api_client.dart` | Fake handlers and call recording |
| Change | `app/lib/copy.dart` | Strings below |
| Change | `app/lib/routes.dart` and `app/lib/main.dart` | Routes `/invite`, `/auth/result`, `/request-invite`, `/settings/accounts` placeholder; provide `Browser` |
| Change | `app/pubspec.yaml` | Add `web` (latest 1.x) to `dependencies` |
| Create | `app/test/support/fake_browser.dart` | `FakeBrowser` recording every call |
| Create | `app/test/screens/sign_in_test.dart`, `app/test/screens/request_invite_test.dart`, `app/test/unit/auth_models_test.dart`, `app/test/unit/format_test.dart` | Tests below |

## Types and signatures

```dart
// Relies on T-007 (use T-007's names if they differ and say so in the PR):
// ApiClient (lib/api/api_client.dart), HttpApiClient, FakeApiClient (records calls in `calls`),
// ApiException {int status; String code; String requestId; String? mailboxId; int? retryAfterSeconds},
// NetworkException, Session {SessionState state; String? csrfToken; ...; String? pendingInviteEmail;
// List<Mailbox> mailboxes; DateTime? stepUpValidUntil}, SessionModel extends ChangeNotifier
// {Session? session; Future<void> refresh(); void wipe();}, Copy (lib/copy.dart), Routes (lib/routes.dart).

// lib/api/models/auth.dart
enum AuthIntent { signIn, join, link, reconnect, stepUp }
extension AuthIntentWire on AuthIntent {
  String get wire => switch (this) {
        AuthIntent.signIn => 'sign_in',
        AuthIntent.join => 'join',
        AuthIntent.link => 'link',
        AuthIntent.reconnect => 'reconnect',
        AuthIntent.stepUp => 'step_up',
      };
}

enum AuthOutcome {
  signedIn, joined, linked, reconnected, steppedUp, notInvited, notRegistered, inviteInvalid,
  emailMismatch, emailUnverified, mailboxLinkedElsewhere, stepUpWrongAccount, consentBlocked,
  cancelled, failed, unknown,
}
AuthOutcome parseAuthOutcome(String? raw); // exact S7 3.4 strings; anything else -> unknown

// lib/api/api_client.dart (added)
abstract class ApiClient {
  /// POST /api/v1/auth/google/start. Returns authorization_url.
  Future<Uri> startAuth({required AuthIntent intent, String? inviteToken, String? mailboxId});
  /// POST /api/v1/invite-requests with body {} (expects 202).
  Future<void> createInviteRequest();
}

// lib/platform/browser.dart
abstract class Browser {
  void assign(Uri url);                    // same-tab navigation (window.location.assign)
  void openExternal(Uri url);              // new tab with 'noopener,noreferrer'
  BrowserPopup? openPopup(String name);    // about:blank popup; call synchronously in a tap handler; null if blocked
  bool get isPopupWindow;                  // window.name == 'mt_step_up' (used by T-1001b)
  void closeWindow();                      // window.close()
  void replaceAddress(String hashPath);    // history.replaceState to '#$hashPath'
}
abstract class BrowserPopup { void navigate(Uri url); void close(); }

/// Returns the URI only if it is https, or http on localhost/127.0.0.1 when [allowLoopbackHttp].
Uri? safeNavigationTarget(String raw, {required bool allowLoopbackHttp});
const bool kE2eBuild = bool.fromEnvironment('MT_E2E'); // true only in e2e builds (T-1101a)

// lib/format.dart
String formatCount(int n);            // 12431 -> "12,431"
String formatDate(DateTime utc);      // local date, "3 Oct 2026"
String formatDateTime(DateTime utc);  // local, "3 Oct 2026, 2:09 pm"

// lib/state/sign_in_model.dart
enum SignInStatus { idle, redirecting, error }
class SignInModel extends ChangeNotifier {
  SignInModel({required ApiClient api, required Browser browser, bool allowLoopbackHttp = kE2eBuild});
  SignInStatus get status;
  String? get errorText;        // a Copy string, or null
  bool get arrivedFromInvite;
  void acceptInviteToken(String raw);   // validates; invalid -> error inviteInvalid
  void showOutcome(AuthOutcome outcome);
  void resetToDefault();                // after sign-out or a 401 wipe
  Future<void> continueWithGoogle();    // intent join, with the invite token when present
}
```

## Algorithm

1. **Routes.** Keep Flutter's default hash URL strategy (the server redirects to `/#/auth/result`). Register `/invite` (query `t`), `/auth/result` (query `outcome`), `/request-invite` and, if T-007 has no such route, `/settings/accounts` as a placeholder that shows the Settings tab (T-1006a replaces its builder). Parse `RouteSettings.name` with `Uri.parse`.
2. **Invite link** (`/invite?t=...`): read `t`. Valid means 43 to 64 characters of `[A-Za-z0-9_-]` (OpenAPI `invite_token`). Call `browser.replaceAddress('/')` at once so the token leaves the address bar and history. Valid: `acceptInviteToken` keeps it in memory only and the Sign-in screen shows the invited state. Invalid: Sign-in shows `Copy.inviteInvalid`.
3. **Continue with Google:** ignore taps while `redirecting`. Set `redirecting`, then `startAuth(intent: AuthIntent.join, inviteToken: token)`. `join` covers every case `[DEFAULT]`: an existing user signs in, a valid invite creates the user, and an address with no account goes to Request invite (S7 3.4), so the default screen never needs `sign_in`. Pass the result through `safeNavigationTarget`; null means `error` with `Copy.signInFailed`; otherwise `browser.assign(url)` and stay `redirecting`. On `ApiException` with code `rate_limited` show `Copy.tooManyRequests`; on any other error or `NetworkException` show `Copy.signInFailed` with a "Try again" button.
4. **Auth result** (`/auth/result?outcome=x`): `parseAuthOutcome`, then `await session.refresh()`, then:
   - `signedIn`, `joined`: replace the whole stack with the Feed route.
   - `notInvited`: replace with Request invite.
   - `notRegistered`, `inviteInvalid`, `emailMismatch`, `emailUnverified`: Sign-in with that copy.
   - `cancelled`, `failed`, `unknown`, `consentBlocked` (v2): Sign-in with `Copy.signInFailed` and retry.
   - `linked`, `mailboxLinkedElsewhere`: replace with `/settings/accounts`, passing the `AuthOutcome` as the route argument (T-1006a shows the message).
   - `reconnected`: replace with the Feed `[DEFAULT]` (the page reloaded, so "back where the user was" is the Feed).
   - `steppedUp`, `stepUpWrongAccount`: replace with the Feed for now; T-1001b changes this branch for the step-up popup.
   Then `browser.replaceAddress('/')` so a reload does not replay the outcome.
5. **Request invite** (session state `pending_invite_request`): show `session.pendingInviteEmail` as plain `Text`, `Copy.inviteOnlyExplainer`, and two buttons. "Request an invite" calls `createInviteRequest`: success shows `Copy.requestSent` and hides the button; `rate_limited` shows `Copy.tooManyRequests`; anything else shows `Copy.actionFailed`. "Use a different account" calls `api.signOut()` (ignore its errors), `session.wipe()`, `signInModel.resetToDefault()` and replaces the stack with Sign-in.
6. **Signed out** (AU-07 AC1): T-007 wipes memory and routes to Sign-in on any `401`. Sign-in must then show the default state: call `resetToDefault()` from the Sign-in route builder whenever the session is not `authenticated` and no outcome or invite token is being shown.
7. **Scopes explainer:** under the button show `Copy.scopesExplainer` `[DEFAULT]` (ASVS V10.7.2 needs scopes explained before the redirect; S9 has no copy).
8. **Browser web implementation:** `assign` uses `window.location.assign`; `openExternal` uses `window.open(url, '_blank', 'noopener,noreferrer')`; `openPopup(name)` uses `window.open('about:blank', name, 'popup,width=500,height=650')` and returns null when the result is null; `replaceAddress` uses `window.history.replaceState(null, '', '#$hashPath')`. Export with `export 'browser_stub.dart' if (dart.library.js_interop) 'browser_web.dart';`.
9. **Format helpers:** `formatCount` inserts commas every three digits; `formatDate` uses month abbreviations Jan to Dec and `toLocal()`; `formatDateTime` uses 12-hour time with lower-case "am"/"pm". No `intl` package.

Copy to add to `lib/copy.dart` (S9 verbatim unless marked):

| Constant | Text |
| --- | --- |
| `productName` | Mail Tinder |
| `tagline` `[DEFAULT]` | Swipe through your inbox and clear the mail you don't want. |
| `continueWithGoogle` | Continue with Google |
| `privacyNotice` `[DEFAULT]` | Privacy notice (opens `/privacy.html` with `openExternal`; the page itself waits on S14) |
| `scopesExplainer` `[DEFAULT]` | Google will ask you to let Mail Tinder read and organise your Gmail, send unsubscribe emails for you, and keep its own settings file in your Google Drive. |
| `invited` | You've been invited. Continue with the Google account the invite was sent to. |
| `inviteInvalid` | This invite link no longer works. Ask for a new invite. |
| `emailMismatch` | This account isn't the one you were invited with. Try the invited account, or ask for a new invite. |
| `emailUnverified` | We couldn't confirm this account's email address. Try another account. |
| `notRegistered` | There's no Mail Tinder account for this Google account. Use your invite link, or request an invite. |
| `signInFailed` | Sign-in didn't finish. Try again. |
| `tryAgain` `[DEFAULT]` | Try again |
| `inviteOnlyExplainer` `[DEFAULT]` | Mail Tinder is invite only. Send a request and you'll hear back by email if it's approved. |
| `requestInvite` | Request an invite |
| `useDifferentAccount` | Use a different account |
| `requestSent` | Request sent. You'll get an email if it's approved. |
| `tooManyRequests` | Too many requests. Try again later. |
| `actionFailed` | Couldn't do that. Try again. |

## Acceptance criteria

| ID | Behaviour (one line) |
| --- | --- |
| AU-02 AC1 | A `not_invited` outcome shows the Request invite screen |
| AU-02 AC4 | A rate-limited invite request shows "Too many requests. Try again later." |
| AU-03 AC1 | The invite token from the link travels with the OAuth start request, and `joined` lands on the Feed |
| AU-03 AC2 | `email_mismatch` shows the S9 mismatch copy |
| AU-03 AC3 | `email_unverified` shows the S9 unverified copy |
| AU-03 AC4 | `signed_in` lands on the Feed with no invite token needed |
| AU-03 AC6 | `invite_invalid` and a malformed invite link show "This invite link no longer works." |
| AU-07 AC1 | After a `401` wipe the Sign-in screen shows its default state |
| XC-03 | Every control on these screens has a Semantics label and meets the tap target guidelines |

## Tests that must pass

- `'AU-03 AC1 invite token from the link is sent with the join request'` (widget)
- `'AU-03 AC1 joined outcome lands on the Feed'` (widget)
- `'AU-03 AC4 signed_in outcome lands on the Feed'` (widget)
- `'AU-02 AC1 not_invited outcome opens Request invite'` (widget)
- `'AU-03 AC6 invite_invalid outcome shows the no longer works message'` (widget)
- `'AU-03 AC6 malformed invite link shows the no longer works message'` (widget)
- `'AU-03 AC2 email_mismatch outcome shows the mismatch message'` (widget)
- `'AU-03 AC3 email_unverified outcome shows the unverified message'` (widget)
- `'AU-07 AC1 after a 401 the Sign-in screen shows its default state'` (widget)
- `'AU-02 AC4 rate limited request shows Too many requests'` (widget)
- `'XC-03 sign-in and request invite controls are labelled'` (widget, `meetsGuideline(labeledTapTargetGuideline)` and `androidTapTargetGuideline`)
- `'s9_sign_in_default shows name, tagline, button and privacy link'` (widget)
- `'s9_sign_in_invited shows the invited message'` (widget)
- `'s9_sign_in_redirecting disables the button and shows a spinner'` (widget)
- `'s9_sign_in_provider_error cancelled and failed show retry'` (widget)
- `'s9_sign_in_not_registered shows the S9 copy'` (widget)
- `'s9_request_invite_default shows the signed-in address'` (widget)
- `'s9_request_invite_submitted shows Request sent'` (widget)
- `'s9_request_invite_use_different_account signs out and shows Sign-in'` (widget)
- `'invite token is removed from the address bar'` (widget, `FakeBrowser.replaceAddress` called with `/`)
- `'start URL that is not https is never navigated to'` (unit, `safeNavigationTarget`)
- `'parseAuthOutcome maps every S7 outcome and unknown'` (unit)
- `'formatCount and formatDate produce the expected text'` (unit)

## Edge cases and traps

- The invite token lives in `SignInModel` memory only: never in `localStorage`, `sessionStorage`, IndexedDB, logs, `debugPrint` or a Semantics label.
- `package:web` does not compile on the VM: import it only from `browser_web.dart` behind the conditional export, or `flutter test` breaks.
- Never navigate to a URL the server did not mark https; loopback http is allowed only when built with `--dart-define=MT_E2E=true`.
- `startAuth` is a `POST` with `X-CSRF-Token` (T-007 adds it); never build a `GET` to `/auth/google/start`.
- Ignore a second tap while redirecting; a double `startAuth` rotates the `pre_auth` record and breaks the first redirect.
- Map outcome strings with an explicit `switch` and an `unknown` result; never default an unknown outcome to success.
- Do not call `usePathUrlStrategy()`: the server redirects to `/#/auth/result`.
- Render `pendingInviteEmail` with `Text`, never as rich text or HTML.
- "Continue with Microsoft" and the consent-blocked state are v2: do not build them.
- Wrap `api.signOut()` in "Use a different account" so a network failure still wipes memory and shows Sign-in.

## Out of scope

- The Confirm it's you overlay and the popup branch of the auth result screen (T-1001b).
- The Connected accounts screen and its outcome messages (T-1006a); Sign out in Settings (T-1006a).
- The privacy notice page itself (S14).

## Done when

- The tests above pass and every required check is green (S10 10.1).
- Definition of done in S10 10.4.
- The PR notes the scopes explainer line as the evidence for ASVS V10.7.2 (`review`).
