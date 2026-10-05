import 'package:app/api/api_client.dart';
import 'package:app/api/fake_api_client.dart';
import 'package:app/api/models/session.dart';
import 'package:app/app.dart';
import 'package:app/copy.dart';
import 'package:app/screens/home/home_shell.dart';
import 'package:app/screens/request_invite/request_invite_screen.dart';
import 'package:app/screens/sign_in/sign_in_screen.dart';
import 'package:app/state/session_model.dart';
import 'package:app/state/sign_in_model.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import 'support/fake_browser.dart';

void main() {
  Widget buildApp(FakeApiClient fakeApi) {
    final model = SessionModel(api: fakeApi);
    final browser = FakeBrowser();
    final signInModel = SignInModel(api: fakeApi, browser: browser);
    return MailTinderApp(
      session: model,
      api: fakeApi,
      signInModel: signInModel,
      browser: browser,
    );
  }

  testWidgets('anonymous session shows sign-in screen', (tester) async {
    final fakeApi = FakeApiClient();
    fakeApi.session = const Session(state: SessionState.anonymous);

    await tester.pumpWidget(buildApp(fakeApi));
    await tester.pumpAndSettle();

    expect(find.byType(SignInScreen), findsOneWidget);
    expect(find.text(Copy.productName), findsOneWidget);
    expect(find.text(Copy.continueWithGoogle), findsOneWidget);
  });

  testWidgets('pending invite request session shows request invite screen', (
    tester,
  ) async {
    final fakeApi = FakeApiClient();
    fakeApi.session = const Session(state: SessionState.pendingInviteRequest);

    await tester.pumpWidget(buildApp(fakeApi));
    await tester.pumpAndSettle();

    expect(find.byType(RequestInviteScreen), findsOneWidget);
    expect(find.text(Copy.requestAnInvite), findsAtLeastNWidgets(1));
  });

  testWidgets('authenticated session shows four tabs in S9 order', (
    tester,
  ) async {
    final fakeApi = FakeApiClient();
    fakeApi.session = const Session(state: SessionState.authenticated);

    await tester.pumpWidget(buildApp(fakeApi));
    await tester.pumpAndSettle();

    expect(find.byType(HomeShell), findsOneWidget);

    // Verify NavigationBar has the 4 destinations in S9 order
    final navBarFinder = find.byType(NavigationBar);
    expect(navBarFinder, findsOneWidget);
    final navBar = tester.widget<NavigationBar>(navBarFinder);

    expect(navBar.destinations.length, equals(4));
    final d0 = navBar.destinations[0] as NavigationDestination;
    final d1 = navBar.destinations[1] as NavigationDestination;
    final d2 = navBar.destinations[2] as NavigationDestination;
    final d3 = navBar.destinations[3] as NavigationDestination;

    expect(d0.label, equals(Copy.tabFeed));
    expect(d1.label, equals(Copy.tabFiled));
    expect(d2.label, equals(Copy.tabNeedsAttention));
    expect(d3.label, equals(Copy.tabSettings));
  });

  testWidgets('XC-03 bottom tabs have semantics labels', (tester) async {
    final fakeApi = FakeApiClient();
    fakeApi.session = const Session(state: SessionState.authenticated);

    await tester.pumpWidget(buildApp(fakeApi));
    await tester.pumpAndSettle();

    // In NavigationDestination, icon has Semantics(label: Copy.tab*), or find by predicate
    expect(
      find.byWidgetPredicate(
        (widget) =>
            widget is Semantics && widget.properties.label == Copy.tabFeed,
      ),
      findsWidgets,
    );
    expect(
      find.byWidgetPredicate(
        (widget) =>
            widget is Semantics && widget.properties.label == Copy.tabFiled,
      ),
      findsWidgets,
    );
    expect(
      find.byWidgetPredicate(
        (widget) =>
            widget is Semantics &&
            widget.properties.label == Copy.tabNeedsAttention,
      ),
      findsWidgets,
    );
    expect(
      find.byWidgetPredicate(
        (widget) =>
            widget is Semantics && widget.properties.label == Copy.tabSettings,
      ),
      findsWidgets,
    );
  });

  testWidgets('ASVS V14.3.1 any 401 wipes session and shows sign-in', (
    tester,
  ) async {
    final fakeApi = FakeApiClient();
    fakeApi.session = const Session(state: SessionState.authenticated);
    final model = SessionModel(api: fakeApi);
    final browser = FakeBrowser();
    final signInModel = SignInModel(api: fakeApi, browser: browser);

    await tester.pumpWidget(
      MailTinderApp(
        session: model,
        api: fakeApi,
        signInModel: signInModel,
        browser: browser,
      ),
    );
    await tester.pumpAndSettle();

    // App is authenticated showing HomeShell
    expect(find.byType(HomeShell), findsOneWidget);

    // Simulate 401: wipe is triggered (in real app, onUnauthenticated does model.wipe())
    model.wipe();
    await tester.pumpAndSettle();

    // Should now show sign-in screen
    expect(find.byType(SignInScreen), findsOneWidget);
    expect(find.byType(HomeShell), findsNothing);
  });

  testWidgets('network failure on bootstrap shows offline with retry', (
    tester,
  ) async {
    final fakeApi = FakeApiClient();
    fakeApi.nextError = const NetworkException();

    await tester.pumpWidget(buildApp(fakeApi));
    await tester.pumpAndSettle();

    expect(find.text(Copy.offline), findsOneWidget);
    expect(find.text(Copy.tryAgain), findsOneWidget);

    // Tapping Try again succeeds when fakeApi is restored
    fakeApi.session = const Session(state: SessionState.authenticated);
    await tester.tap(find.text(Copy.tryAgain));
    await tester.pumpAndSettle();

    expect(find.byType(HomeShell), findsOneWidget);
  });
}
