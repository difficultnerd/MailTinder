import 'package:app/api/api_client.dart';
import 'package:app/api/fake_api_client.dart';
import 'package:app/api/models/session.dart';
import 'package:app/app.dart';
import 'package:app/copy.dart';
import 'package:app/screens/request_invite/request_invite_screen.dart';
import 'package:app/screens/sign_in/sign_in_screen.dart';
import 'package:app/state/session_model.dart';
import 'package:app/state/sign_in_model.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import '../support/fake_browser.dart';

void main() {
  Widget buildApp(
    FakeApiClient fakeApi, {
    FakeBrowser? browser,
    SessionModel? session,
  }) {
    final b = browser ?? FakeBrowser();
    final s = session ?? SessionModel(api: fakeApi);
    return MailTinderApp(
      session: s,
      api: fakeApi,
      signInModel: SignInModel(api: fakeApi, browser: b),
      browser: b,
    );
  }

  testWidgets('s9_request_invite_default shows the signed-in address', (
    tester,
  ) async {
    final fakeApi = FakeApiClient();
    fakeApi.session = const Session(
      state: SessionState.pendingInviteRequest,
      pendingInviteEmail: 'james@example.com',
    );

    await tester.pumpWidget(buildApp(fakeApi));
    await tester.pumpAndSettle();

    expect(find.byType(RequestInviteScreen), findsOneWidget);
    expect(find.text('james@example.com'), findsOneWidget);
    expect(find.text(Copy.inviteOnlyExplainer), findsOneWidget);
    expect(find.text(Copy.requestAnInvite), findsWidgets);
    expect(find.text(Copy.useDifferentAccount), findsOneWidget);
  });

  testWidgets('s9_request_invite_submitted shows Request sent', (tester) async {
    final fakeApi = FakeApiClient();
    fakeApi.session = const Session(
      state: SessionState.pendingInviteRequest,
      pendingInviteEmail: 'james@example.com',
    );

    await tester.pumpWidget(buildApp(fakeApi));
    await tester.pumpAndSettle();

    await tester.tap(find.text(Copy.requestAnInvite));
    await tester.pumpAndSettle();

    expect(find.text(Copy.requestSent), findsOneWidget);
    // The request button disappears once submitted.
    expect(find.text(Copy.requestAnInvite), findsNothing);
  });

  testWidgets(
    's9_request_invite_use_different_account signs out and shows Sign-in',
    (tester) async {
      final fakeApi = FakeApiClient();
      fakeApi.session = const Session(
        state: SessionState.pendingInviteRequest,
        pendingInviteEmail: 'james@example.com',
      );
      final browser = FakeBrowser();

      await tester.pumpWidget(buildApp(fakeApi, browser: browser));
      await tester.pumpAndSettle();
      expect(find.byType(RequestInviteScreen), findsOneWidget);

      await tester.tap(find.text(Copy.useDifferentAccount));
      await tester.pumpAndSettle();

      // Sign-out called and memory wiped; routes to Sign-in.
      expect(
        fakeApi.calls.any((c) => c.path == '/api/v1/auth/sign-out'),
        isTrue,
      );
      expect(find.byType(SignInScreen), findsOneWidget);
      expect(find.byType(RequestInviteScreen), findsNothing);
    },
  );

  testWidgets('request invite rate limited shows Too many requests', (
    tester,
  ) async {
    final fakeApi = FakeApiClient();
    fakeApi.session = const Session(
      state: SessionState.pendingInviteRequest,
      pendingInviteEmail: 'james@example.com',
    );

    await tester.pumpWidget(buildApp(fakeApi));
    await tester.pumpAndSettle();

    // Set the error AFTER the initial session load so the app reaches the screen.
    fakeApi.nextError = ApiException(
      status: 429,
      code: 'rate_limited',
      requestId: 'r1',
    );
    await tester.tap(find.text(Copy.requestAnInvite));
    await tester.pumpAndSettle();

    expect(find.text(Copy.tooManyRequests), findsOneWidget);
    // Button stays to retry.
    expect(find.text(Copy.requestAnInvite), findsWidgets);
  });
}
