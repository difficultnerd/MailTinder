# T-007: Flutter app shell, routing and ApiClient

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M0 | sonnet | about 450 lines of Dart plus tests | None |

**Read only these spec sections:** S9 "Screen map" and section 9 "Copy rules" (`docs/specs/S9-functional-screens.md`); S10 3.2 (`docs/specs/S10-test-strategy.md`); S7 1, 2 (rows Base path, Format, Caching, Request ID), 3.2 "States" table, 3.3, 4 (error body and the `401`, `403 csrf_failed` rows), 5.2 API-AUTH-3 and API-AUTH-4 (`docs/specs/S7-api-contract.md`); the `Session`, `Mailbox` and `Problem` schemas in `docs/specs/S7-api-contract.openapi.yaml`; S5 "Browser" table; ASVS register rows V14.3.1 and V14.3.3. `docs/backlog/CONVENTIONS.md` "Flutter conventions". Nothing else is needed.

## Goal

The template's "Hello" app becomes the Mail Tinder shell: a session bootstrap through `GET /api/v1/session`, routing by session state (Sign-in, Request invite, or the four bottom tabs Feed, Filed, Needs Attention, Settings with placeholder bodies), one `ApiClient` interface with an HTTP implementation and a fake, and the rule that any `401` wipes in-memory state and returns to Sign-in. Every M10 screen task plugs into this shell; T-1001a adds the auth start, invite and auth result routes.

## Files

| Action | Path | What |
| --- | --- | --- |
| Change | `app/pubspec.yaml` | `name: app` stays; add `http: ^1.2.0` to `dependencies` |
| Change | `app/lib/main.dart` | Builds `HttpApiClient` and `SessionModel`, runs `MailTinderApp` |
| Create | `app/lib/app.dart` | `MailTinderApp`: `MaterialApp`, `onGenerateRoute`, session-driven home |
| Create | `app/lib/routes.dart` | `Routes` constants and `onGenerateRoute` |
| Create | `app/lib/copy.dart` | `Copy` constants used by the shell |
| Create | `app/lib/api/api_client.dart` | `ApiClient`, `ApiException`, `NetworkException` |
| Create | `app/lib/api/http_api_client.dart` | `HttpApiClient` |
| Create | `app/lib/api/fake_api_client.dart` | `FakeApiClient` with call recording |
| Create | `app/lib/api/models/session.dart` | `Session`, `SessionState`, `SessionUser`, `Mailbox`, `MailboxStatus` |
| Create | `app/lib/state/session_model.dart` | `SessionModel extends ChangeNotifier` |
| Create | `app/lib/screens/home/home_shell.dart` | Bottom tabs with placeholder bodies |
| Create | `app/lib/screens/sign_in/sign_in_screen.dart` | Placeholder (title, product line, disabled button); T-1001a completes it |
| Create | `app/lib/screens/request_invite/request_invite_screen.dart` | Placeholder; T-1001a completes it |
| Create | `app/lib/screens/feed/feed_screen.dart`, `filed/filed_screen.dart`, `needs_attention/needs_attention_screen.dart`, `settings/settings_screen.dart` | Placeholders, one `Text` each |
| Delete | `app/test/app_test.dart` | Template test |
| Create | `app/test/api/http_api_client_test.dart`, `app/test/state/session_model_test.dart`, `app/test/app_routing_test.dart`, `app/test/pubspec_policy_test.dart` | Tests below |

## Types and signatures

```dart
// lib/api/api_client.dart
abstract class ApiClient {
  /// GET /api/v1/session. Never 401 (S7 5.2); creates a pre_auth session when none exists.
  Future<Session> getSession();
  /// POST /api/v1/auth/sign-out -> 204.
  Future<void> signOut();
}

class ApiException implements Exception {
  ApiException({required this.status, required this.code, required this.requestId,
      this.mailboxId, this.retryAfterSeconds, this.fields = const []});
  final int status;
  final String code;            // S7 4 `code`; 'unknown' when the body is not a problem document
  final String requestId;       // '' when absent
  final String? mailboxId;
  final int? retryAfterSeconds;
  final List<String> fields;
}
class NetworkException implements Exception { const NetworkException(); }

// lib/api/http_api_client.dart
typedef UnauthenticatedHandler = void Function();
class HttpApiClient implements ApiClient {
  HttpApiClient({required http.Client client, required Uri origin, required this.onUnauthenticated});
  final UnauthenticatedHandler onUnauthenticated;
  String? get csrfToken;                       // from the last getSession
  /// Shared plumbing for every endpoint added by later tasks.
  Future<Map<String, Object?>?> send(String method, String path,
      {Map<String, Object?>? body, String? idempotencyKey, Set<int> expect = const {200}});
}

// lib/api/fake_api_client.dart
class FakeCall { FakeCall(this.method, this.path, [this.body]); final String method; final String path; final Object? body; }
class FakeApiClient implements ApiClient {
  final List<FakeCall> calls = [];
  Session session = Session.anonymous();
  Object? nextError;                           // thrown (and cleared) by the next call when set
}

// lib/api/models/session.dart
enum SessionState { anonymous, preAuth, pendingInviteRequest, authenticated }
enum MailboxStatus { connected, needsSignIn, consentBlocked, unknown }
class SessionUser { final String userId; final bool isAdmin; }
class Mailbox { final String mailboxId; final String provider; final String emailAddress; final MailboxStatus status; }
class Session {
  final SessionState state;
  final String? csrfToken;
  final SessionUser? user;
  final String? pendingInviteEmail;
  final DateTime? stepUpValidUntil;
  final List<Mailbox> mailboxes;
  factory Session.fromJson(Map<String, Object?> json);
  factory Session.anonymous();
}

// lib/state/session_model.dart
class SessionModel extends ChangeNotifier {
  SessionModel({required ApiClient api});
  Session? get session;            // null until the first refresh completes
  bool get loading;
  Object? get lastError;
  Future<void> refresh();          // getSession; on NetworkException keeps session null and sets lastError
  void wipe();                     // drops everything held in memory and notifies
  Future<void> signOut();          // api.signOut (errors ignored), then wipe
}

// lib/routes.dart
abstract final class Routes {
  static const home = '/';
  static const signIn = '/sign-in';
  static const requestInvite = '/request-invite';
}
Route<Object?>? onGenerateRoute(RouteSettings settings, SessionModel session, ApiClient api);

// lib/copy.dart
abstract final class Copy {
  static const productName = 'Mail Tinder';
  static const tabFeed = 'Feed';
  static const tabFiled = 'Filed';
  static const tabNeedsAttention = 'Needs Attention';
  static const tabSettings = 'Settings';
  static const genericError = "Something went wrong. Try again.";   // [DEFAULT] S9 9: say what happened and what to do
  static const offline = "You're offline. Check your connection and try again."; // [DEFAULT]
}
```

## Algorithm

1. **Wire format.** All paths are under `/api/v1` on the app's own origin (`Uri.base.origin` in `main.dart`). Requests send `Accept: application/json`; bodies are JSON with `Content-Type: application/json; charset=utf-8`. Every `POST`, `PUT`, `PATCH` and `DELETE` sends `X-CSRF-Token: <csrfToken>` (S7 3.3); when `idempotencyKey` is given it also sends `Idempotency-Key`. Responses with no body (`204`) return null.
2. **Errors.** A status outside `expect` is parsed as RFC 9457 (`application/problem+json`): read `code`, `request_id`, `mailbox_id`, `retry_after_seconds`, `fields`. Anything unparsable gives `code: 'unknown'`. A thrown `http.ClientException` or `SocketException`-like error becomes `NetworkException`.
3. **401.** Any `401` calls `onUnauthenticated()` before throwing the `ApiException` (S7 5.2: the app clears its in-memory state on any `401`). `main.dart` wires `onUnauthenticated` to `sessionModel.wipe()` followed by `sessionModel.refresh()`.
4. **403 `csrf_failed`.** Refresh the session once (new CSRF token) and retry the same request once; a second `csrf_failed` throws (S7 4 row: "Reload session, retry once").
5. **Session JSON.** `state` strings: `anonymous`, `pre_auth`, `pending_invite_request`, `authenticated`; an unknown value maps to `anonymous`. Mailbox `status`: `connected`, `needs_sign_in`, `consent_blocked`, anything else `unknown`. Times parse with `DateTime.parse(...).toUtc()`.
6. **Bootstrap.** `MailTinderApp` calls `session.refresh()` once in `initState`. While `session == null && loading`: a centred `CircularProgressIndicator` with `Semantics(label: 'Loading')`. On `lastError` with no session: `Copy.offline` and a "Try again" button calling `refresh()`.
7. **Routing by state.** The `home` route builds from `ListenableBuilder(listenable: session)`: `authenticated` gives `HomeShell`; `pendingInviteRequest` gives `RequestInviteScreen`; `anonymous` and `preAuth` give `SignInScreen`. When the state changes (for example a wipe after `401`), the home route rebuilds and `MailTinderApp` pops any pushed routes with `Navigator.popUntil(ModalRoute.withName(Routes.home))`.
8. **URL strategy.** Keep Flutter's default hash strategy; do not call `usePathUrlStrategy()`. The server redirects to `/#/auth/result` (S7 3.4), which T-1001a handles. `onGenerateRoute` parses `settings.name` with `Uri.parse` and switches on `uri.path`; unknown paths fall back to `home`.
9. **HomeShell.** `Scaffold` with a `NavigationBar` of four destinations in S9 order (Feed, Filed, Needs Attention, Settings), each with an icon, the `Copy` label and a `Semantics` label equal to that text; the selected index is held in the shell's `State` (memory only). Each body is its placeholder screen. Tab bodies are kept alive with an `IndexedStack`.
10. **Wipe.** `SessionModel.wipe()` sets `session = null`, clears `lastError`, notifies listeners. Later view models register to be wiped too; leave a `final List<VoidCallback> _onWipe` with `void addWipeListener(VoidCallback f)` so they can.
11. **Memory only.** Do not add `shared_preferences`, `hive`, `sqflite`, `idb_shim`, `flutter_secure_storage` or any storage package, and do not touch `window.localStorage` (S5 "Browser", ASVS V14.3.3).
12. Delete the template test; write the tests below with `FakeApiClient` for widgets and `MockClient` from `package:http/testing.dart` for `HttpApiClient`.

## Acceptance criteria

| ID | Behaviour (one line) |
| --- | --- |
| V14.3.1 | Any `401` clears the app's in-memory session state and returns to Sign-in |
| V14.3.3 | The app depends on no browser storage package, so nothing but the cookie is stored |
| XC-03 | Every tab in the shell has a screen reader label |

## Tests that must pass

- `'ASVS V14.3.1 any 401 wipes session and shows sign-in'` (widget, `app_routing_test.dart`)
- `'ASVS V14.3.1 HttpApiClient calls onUnauthenticated on 401'` (unit, `http_api_client_test.dart`)
- `'ASVS V14.3.3 pubspec has no browser storage packages'` (unit, `pubspec_policy_test.dart`: reads `pubspec.yaml` and fails on any of the step 11 names)
- `'XC-03 bottom tabs have semantics labels'` (widget)
- `'csrf header sent on post and not on get'` (unit)
- `'idempotency key header sent when given'` (unit)
- `'problem json parsed into ApiException'` (unit: code, request_id, mailbox_id, retry_after_seconds)
- `'non problem error body gives code unknown'` (unit)
- `'csrf_failed refreshes session and retries once'` (unit)
- `'client exception becomes NetworkException'` (unit)
- `'session json parses every state and mailbox status'` (unit)
- `'authenticated session shows four tabs in S9 order'` (widget)
- `'pending invite request session shows request invite screen'` (widget)
- `'anonymous session shows sign-in screen'` (widget)
- `'network failure on bootstrap shows offline with retry'` (widget)

## Edge cases and traps

- Tests never touch the network: `HttpApiClient` gets a `MockClient`; widgets get `FakeApiClient`.
- `fromJson` must ignore unknown fields (S7 2: additive fields do not bump the version).
- Do not put the CSRF token, session data or any mail field in a URL, a log line or `debugPrint` (`avoid_print` is on; the privacy rules flag `print(session)`).
- `flutter analyze --fatal-infos` fails on infos, including `prefer_const_constructors` and `prefer_final_locals`; fix them, do not suppress them.
- `strict-casts` and `strict-raw-types` are on: cast JSON values explicitly (`json['state'] as String?`).
- Do not run `flutter create`; the app is web only for now, and new platform folders would add files nobody reviews.
- Do not add a state management package (CONVENTIONS); `ChangeNotifier` and `ListenableBuilder` only.
- On web the browser adds the `Origin` header and sends the `__session` cookie on same-origin requests by itself; do not try to set either.
- Keep copy in `lib/copy.dart`; no string literals for user-facing text in widgets.

## Out of scope

- `startAuth`, the invite and auth result routes, the `Browser` abstraction and the real Sign-in and Request invite screens: T-1001a.
- Step-up overlay: T-1001b. Every tab's real content: T-1002 onwards.
- CSP and `firebase.json`: T-006.

## Done when

- The tests above pass and every required check is green (S10 10.1).
- Definition of done in S10 10.4.
- `flutter build web` succeeds locally.
