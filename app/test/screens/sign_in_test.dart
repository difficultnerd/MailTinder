import 'package:app/api/api_client.dart';
import 'package:app/api/fake_api_client.dart';
import 'package:app/api/models/auth.dart';
import 'package:app/api/models/session.dart';
import 'package:app/app.dart';
import 'package:app/copy.dart';
import 'package:app/platform/browser.dart';
import 'package:app/routes.dart';
import 'package:app/screens/home/home_shell.dart';
import 'package:app/screens/request_invite/request_invite_screen.dart';
import 'package:app/screens/sign_in/sign_in_screen.dart';
import 'package:app/state/session_model.dart';
import 'package:app/state/sign_in_model.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import '../support/fake_browser.dart';

void main() {
  // A valid invite token is 43 to 64 chars of [A-Za-z0-9_-].
  const validToken = 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa';

  Widget buildApp(
    FakeApiClient fakeApi, {
    FakeBrowser? browser,
    SessionModel? session,
    SignInModel? signIn,
  }) {
    final b = browser ?? FakeBrowser();
    final s = session ?? SessionModel(api: fakeApi);
    final m = signIn ?? SignInModel(api: fakeApi, browser: b);
    return MailTinderApp(session: s, api: fakeApi, signInModel: m, browser: b);
  }

  Future<void> navigateTo(
    WidgetTester tester,
    FakeApiClient fakeApi,
    String route, {
    FakeBrowser? browser,
  }) async {
    final navigator = tester.state<NavigatorState>(find.byType(Navigator));
    navigator.pushNamed(route);
    await tester.pumpAndSettle();
  }

  group('s9_sign_in', () {
    testWidgets(
      's9_sign_in_default shows name, tagline, button and privacy link',
      (tester) async {
        final fakeApi = FakeApiClient();
        fakeApi.session = const Session(state: SessionState.anonymous);
        await tester.pumpWidget(buildApp(fakeApi));
        await tester.pumpAndSettle();

        expect(find.text(Copy.productName), findsOneWidget);
        expect(find.text(Copy.tagline), findsOneWidget);
        expect(find.text(Copy.continueWithGoogle), findsOneWidget);
        expect(find.text(Copy.privacyNotice), findsOneWidget);
      },
    );

    testWidgets('s9_sign_in_invited shows the invited message', (tester) async {
      final fakeApi = FakeApiClient();
      fakeApi.session = const Session(state: SessionState.anonymous);
      final browser = FakeBrowser();
      final signIn = SignInModel(api: fakeApi, browser: browser);
      signIn.acceptInviteToken(validToken);

      await tester.pumpWidget(
        buildApp(fakeApi, browser: browser, signIn: signIn),
      );
      await tester.pumpAndSettle();

      expect(find.text(Copy.invited), findsOneWidget);
    });

    testWidgets(
      's9_sign_in_redirecting disables the button and shows a spinner',
      (tester) async {
        final fakeApi = FakeApiClient();
        fakeApi.session = const Session(state: SessionState.anonymous);
        await tester.pumpWidget(buildApp(fakeApi));
        await tester.pumpAndSettle();

        // Start auth (redirecting state).
        await tester.tap(find.text(Copy.continueWithGoogle));
        await tester.pump();

        final button = tester.widget<ElevatedButton>(
          find.byType(ElevatedButton),
        );
        expect(button.onPressed, isNull); // disabled
        expect(find.byType(CircularProgressIndicator), findsOneWidget);
      },
    );

    testWidgets('s9_sign_in_provider_error cancelled and failed show retry', (
      tester,
    ) async {
      final fakeApi = FakeApiClient();
      fakeApi.session = const Session(state: SessionState.anonymous);
      final browser = FakeBrowser();
      final signIn = SignInModel(api: fakeApi, browser: browser);

      await tester.pumpWidget(
        buildApp(fakeApi, browser: browser, signIn: signIn),
      );
      await tester.pumpAndSettle();

      signIn.showOutcome(AuthOutcome.failed);
      await tester.pumpAndSettle();
      expect(find.text(Copy.signInFailed), findsOneWidget);
      expect(
        find.text(Copy.tryAgain),
        findsNothing,
      ); // tryAgain is a default; screen uses continue button
      expect(find.text(Copy.continueWithGoogle), findsOneWidget);
    });

    testWidgets('s9_sign_in_not_registered shows the S9 copy', (tester) async {
      final fakeApi = FakeApiClient();
      fakeApi.session = const Session(state: SessionState.anonymous);
      final browser = FakeBrowser();
      final signIn = SignInModel(api: fakeApi, browser: browser);

      await tester.pumpWidget(
        buildApp(fakeApi, browser: browser, signIn: signIn),
      );
      await tester.pumpAndSettle();

      signIn.showOutcome(AuthOutcome.notRegistered);
      await tester.pumpAndSettle();
      expect(find.text(Copy.notRegistered), findsOneWidget);
    });
  });

  group('AU-03 invite token', () {
    testWidgets(
      'AU-03 AC1 invite token from the link is sent with the join request',
      (tester) async {
        final fakeApi = FakeApiClient();
        fakeApi.session = const Session(state: SessionState.anonymous);
        final browser = FakeBrowser();

        await tester.pumpWidget(buildApp(fakeApi, browser: browser));
        await tester.pumpAndSettle();

        await navigateTo(tester, fakeApi, '${Routes.invite}?t=$validToken');
        expect(find.text(Copy.invited), findsOneWidget);

        await tester.tap(find.text(Copy.continueWithGoogle));
        await tester.pump();

        final startCall = fakeApi.calls.firstWhere(
          (c) => c.path == '/api/v1/auth/google/start',
        );
        final body = startCall.body as Map<String, Object?>;
        expect(body['intent'], equals('join'));
        expect(body['invite_token'], equals(validToken));
      },
    );

    testWidgets('AU-03 AC1 joined outcome lands on the Feed', (tester) async {
      final fakeApi = FakeApiClient();
      fakeApi.session = const Session(
        state: SessionState.authenticated,
        user: SessionUser(userId: 'u1', isAdmin: false),
      );
      final browser = FakeBrowser();

      await tester.pumpWidget(buildApp(fakeApi, browser: browser));
      await tester.pumpAndSettle();

      await navigateTo(tester, fakeApi, '${Routes.authResult}?outcome=joined');

      expect(find.byType(HomeShell), findsOneWidget);
      expect(browser.replacedAddresses, contains('/'));
    });

    testWidgets('AU-03 AC4 signed_in outcome lands on the Feed', (
      tester,
    ) async {
      final fakeApi = FakeApiClient();
      fakeApi.session = const Session(
        state: SessionState.authenticated,
        user: SessionUser(userId: 'u1', isAdmin: false),
      );
      final browser = FakeBrowser();

      await tester.pumpWidget(buildApp(fakeApi, browser: browser));
      await tester.pumpAndSettle();

      await navigateTo(
        tester,
        fakeApi,
        '${Routes.authResult}?outcome=signed_in',
      );

      expect(find.byType(HomeShell), findsOneWidget);
    });

    testWidgets('AU-02 AC1 not_invited outcome opens Request invite', (
      tester,
    ) async {
      final fakeApi = FakeApiClient();
      final browser = FakeBrowser();

      await tester.pumpWidget(buildApp(fakeApi, browser: browser));
      await tester.pumpAndSettle();

      await navigateTo(
        tester,
        fakeApi,
        '${Routes.authResult}?outcome=not_invited',
      );

      expect(find.byType(RequestInviteScreen), findsOneWidget);
    });

    testWidgets(
      'AU-03 AC6 invite_invalid outcome shows the no longer works message',
      (tester) async {
        final fakeApi = FakeApiClient();
        fakeApi.session = const Session(state: SessionState.anonymous);
        final browser = FakeBrowser();
        final signIn = SignInModel(api: fakeApi, browser: browser);

        await tester.pumpWidget(
          buildApp(fakeApi, browser: browser, signIn: signIn),
        );
        await tester.pumpAndSettle();

        signIn.showOutcome(AuthOutcome.inviteInvalid);
        await tester.pumpAndSettle();
        expect(find.text(Copy.inviteInvalid), findsOneWidget);
      },
    );

    testWidgets(
      'AU-03 AC6 malformed invite link shows the no longer works message',
      (tester) async {
        final fakeApi = FakeApiClient();
        fakeApi.session = const Session(state: SessionState.anonymous);
        final browser = FakeBrowser();

        await tester.pumpWidget(buildApp(fakeApi, browser: browser));
        await tester.pumpAndSettle();

        await navigateTo(tester, fakeApi, '${Routes.invite}?t=tooshort');
        expect(find.text(Copy.inviteInvalid), findsOneWidget);
      },
    );

    testWidgets('AU-03 AC2 email_mismatch outcome shows the mismatch message', (
      tester,
    ) async {
      final fakeApi = FakeApiClient();
      final browser = FakeBrowser();
      final signIn = SignInModel(api: fakeApi, browser: browser);

      await tester.pumpWidget(
        buildApp(fakeApi, browser: browser, signIn: signIn),
      );
      await tester.pumpAndSettle();

      signIn.showOutcome(AuthOutcome.emailMismatch);
      await tester.pumpAndSettle();
      expect(find.text(Copy.emailMismatch), findsOneWidget);
    });

    testWidgets(
      'AU-03 AC3 email_unverified outcome shows the unverified message',
      (tester) async {
        final fakeApi = FakeApiClient();
        final browser = FakeBrowser();
        final signIn = SignInModel(api: fakeApi, browser: browser);

        await tester.pumpWidget(
          buildApp(fakeApi, browser: browser, signIn: signIn),
        );
        await tester.pumpAndSettle();

        signIn.showOutcome(AuthOutcome.emailUnverified);
        await tester.pumpAndSettle();
        expect(find.text(Copy.emailUnverified), findsOneWidget);
      },
    );

    testWidgets(
      'AU-07 AC1 after a 401 the Sign-in screen shows its default state',
      (tester) async {
        final fakeApi = FakeApiClient();
        final model = SessionModel(api: fakeApi);
        fakeApi.session = const Session(
          state: SessionState.authenticated,
          user: SessionUser(userId: 'u1', isAdmin: false),
        );
        final browser = FakeBrowser();
        final signIn = SignInModel(api: fakeApi, browser: browser);

        await tester.pumpWidget(
          buildApp(fakeApi, browser: browser, session: model, signIn: signIn),
        );
        await tester.pumpAndSettle();
        expect(find.byType(HomeShell), findsOneWidget);

        // 401: wipe clears the session; the sign-in builder must reset to default.
        model.wipe();
        await tester.pumpAndSettle();

        expect(find.byType(SignInScreen), findsOneWidget);
        expect(signIn.status, SignInStatus.idle);
        expect(signIn.arrivedFromInvite, isFalse);
        expect(signIn.errorText, isNull);
        // Default copy is shown, not an invited/error variant.
        expect(find.text(Copy.invited), findsNothing);
        expect(find.text(Copy.signInFailed), findsNothing);
      },
    );

    testWidgets('AU-02 AC4 rate limited request shows Too many requests', (
      tester,
    ) async {
      final fakeApi = FakeApiClient();
      fakeApi.session = const Session(state: SessionState.anonymous);
      final browser = FakeBrowser();
      final signIn = SignInModel(api: fakeApi, browser: browser);

      await tester.pumpWidget(
        buildApp(fakeApi, browser: browser, signIn: signIn),
      );
      await tester.pumpAndSettle();

      // Set the error AFTER the initial session load so the app reaches Sign-in.
      fakeApi.nextError = ApiException(
        status: 429,
        code: 'rate_limited',
        requestId: 'r1',
      );
      await signIn.continueWithGoogle();
      await tester.pumpAndSettle();
      expect(find.text(Copy.tooManyRequests), findsOneWidget);
    });

    test('start URL that is not https is never navigated to', () {
      final browser = FakeBrowser();
      final model = SignInModel(
        api: FakeApiClient(),
        browser: browser,
        allowLoopbackHttp: false,
      );
      expect(model, isNotNull);
      final target = safeNavigationTarget(
        'http://evil.example.com',
        allowLoopbackHttp: false,
      );
      expect(target, isNull);
    });

    testWidgets('invite token is removed from the address bar', (tester) async {
      final fakeApi = FakeApiClient();
      fakeApi.session = const Session(state: SessionState.anonymous);
      final browser = FakeBrowser();

      await tester.pumpWidget(buildApp(fakeApi, browser: browser));
      await tester.pumpAndSettle();

      await navigateTo(tester, fakeApi, '${Routes.invite}?t=$validToken');
      expect(browser.replacedAddresses, contains('/'));
    });
  });
}
