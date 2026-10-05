import 'package:app/api/api_client.dart';
import 'package:app/api/fake_api_client.dart';
import 'package:app/api/models/auth.dart';
import 'package:app/api/models/session.dart';
import 'package:app/app.dart';
import 'package:app/copy.dart';
import 'package:app/platform/browser.dart';
import 'package:app/screens/sign_in/auth_result_screen.dart';
import 'package:app/state/session_model.dart';
import 'package:app/state/sign_in_model.dart';
import 'package:app/state/step_up_controller.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import '../support/fake_browser.dart';

void main() {
  const authenticated = Session(
    state: SessionState.authenticated,
    user: SessionUser(userId: 'u1', isAdmin: false),
  );

  ApiException stepUpRequired() => ApiException(
    status: 403,
    code: 'step_up_required',
    requestId: 'r1',
  );

  Widget buildApp(
    FakeApiClient fakeApi, {
    FakeBrowser? browser,
    SessionModel? session,
    StepUpController? stepUp,
    DateTime Function()? now,
  }) {
    final b = browser ?? FakeBrowser();
    final s = session ?? SessionModel(api: fakeApi);
    final signIn = SignInModel(api: fakeApi, browser: b);
    final su =
        stepUp ??
        StepUpController(
          api: fakeApi,
          session: s,
          browser: b,
          now: now,
          pollEvery: const Duration(milliseconds: 10),
          giveUpAfter: const Duration(minutes: 5),
        );
    return MailTinderApp(
      session: s,
      api: fakeApi,
      signInModel: signIn,
      browser: b,
      stepUp: su,
    );
  }

  group('s9_step_up', () {
    testWidgets(
      's9_step_up_prompt shows Confirm it\'s you and the waiting action',
      (tester) async {
        final fakeApi = FakeApiClient();
        fakeApi.session = authenticated;
        final controller = StepUpController(
          api: fakeApi,
          session: SessionModel(api: fakeApi),
          browser: FakeBrowser(),
        );
        await tester.pumpWidget(buildApp(fakeApi, stepUp: controller));
        await tester.pumpAndSettle();

        fakeApi.nextError = stepUpRequired();
        final future = controller.run(
          waitingActionLabel: 'to disconnect jane@example.com',
          action: () async { await fakeApi.signOut(); return 'ok'; },
        );
        await tester.pumpAndSettle();

        expect(find.text(Copy.confirmItsYou), findsOneWidget);
        expect(find.text('to disconnect jane@example.com'), findsOneWidget);
        expect(find.text(Copy.stepUpExplainer), findsOneWidget);
        expect(find.text(Copy.continueWithGoogle), findsOneWidget);
        expect(find.text(Copy.cancel), findsOneWidget);

        controller.cancel();
        await tester.pumpAndSettle();
        await future;
      },
    );

    testWidgets(
      's9_step_up_redirecting opens the popup and disables Continue',
      (tester) async {
        final fakeApi = FakeApiClient();
        fakeApi.session = authenticated;
        final browser = FakeBrowser();
        final controller = StepUpController(
          api: fakeApi,
          session: SessionModel(api: fakeApi),
          browser: browser,
        );
        await tester.pumpWidget(buildApp(fakeApi, browser: browser, stepUp: controller));
        await tester.pumpAndSettle();

        fakeApi.nextError = stepUpRequired();
        final future = controller.run(
          waitingActionLabel: 'to disconnect jane@example.com',
          action: () async { await fakeApi.signOut(); return 'ok'; },
        );
        await tester.pumpAndSettle();

        await tester.tap(find.text(Copy.continueWithGoogle));
        await tester.pump();
        await tester.pump(const Duration(milliseconds: 50));

        expect(browser.openedPopups, contains('mt_step_up'));
        expect(controller.status, StepUpStatus.redirecting);
        final button = tester.widget<ElevatedButton>(
          find.byType(ElevatedButton),
        );
        expect(button.onPressed, isNull); // disabled
        expect(find.byType(CircularProgressIndicator), findsWidgets);

        controller.cancel();
        await tester.pumpAndSettle();
        await future;
      },
    );

    testWidgets(
      's9_step_up_confirmed reruns the waiting action once without a second tap',
      (tester) async {
        final fakeApi = FakeApiClient();
        fakeApi.session = authenticated;
        final browser = FakeBrowser();
        final controller = StepUpController(
          api: fakeApi,
          session: SessionModel(api: fakeApi),
          browser: browser,
        );
        await tester.pumpWidget(buildApp(fakeApi, browser: browser, stepUp: controller));
        await tester.pumpAndSettle();

        var actionCalls = 0;
        fakeApi.nextError = stepUpRequired();
        final future = controller.run(
          waitingActionLabel: 'to disconnect jane@example.com',
          action: () async {
            actionCalls++;
            await fakeApi.signOut();
            return 'ok';
          },
        );
        await tester.pumpAndSettle();

        await tester.tap(find.text(Copy.continueWithGoogle));
        await tester.pump();
        await tester.pump(const Duration(milliseconds: 50));

        // Fake session flips stepUpValidUntil to a fresh value.
        fakeApi.session = Session(
          state: SessionState.authenticated,
          user: SessionUser(userId: 'u1', isAdmin: false),
          stepUpValidUntil: _afterNow(),
        );
        await tester.pump(const Duration(milliseconds: 50));
        await tester.pumpAndSettle();

        final result = await future;
        expect(result, 'ok');
        expect(actionCalls, 2); // original + one re-run
        expect(controller.status, StepUpStatus.idle);
        expect(find.text(Copy.confirmItsYou), findsNothing);
      },
    );

    testWidgets(
      's9_step_up_wrong_account popup page shows the not linked message',
      (tester) async {
        final fakeApi = FakeApiClient();
        fakeApi.session = authenticated;
        final browser = FakeBrowser()..popupWindow = true;
        final controller = StepUpController(
          api: fakeApi,
          session: SessionModel(api: fakeApi),
          browser: browser,
        );
        await tester.pumpWidget(buildApp(fakeApi, browser: browser, stepUp: controller));
        await tester.pumpAndSettle();

        final navigator = tester.state<NavigatorState>(find.byType(Navigator));
        navigator.pushNamed(
          '/auth/result?outcome=step_up_wrong_account',
        );
        await tester.pumpAndSettle();

        expect(find.text(Copy.stepUpWrongAccount), findsOneWidget);
        expect(find.text(Copy.close), findsOneWidget);
        expect(find.byType(AuthResultScreen), findsOneWidget);
      },
    );

    testWidgets(
      's9_step_up_cancelled shows Not confirmed and leaves the screen unchanged',
      (tester) async {
        final fakeApi = FakeApiClient();
        fakeApi.session = authenticated;
        final controller = StepUpController(
          api: fakeApi,
          session: SessionModel(api: fakeApi),
          browser: FakeBrowser(),
        );
        await tester.pumpWidget(buildApp(fakeApi, stepUp: controller));
        await tester.pumpAndSettle();

        fakeApi.nextError = stepUpRequired();
        final future = controller.run(
          waitingActionLabel: 'to disconnect jane@example.com',
          action: () async { await fakeApi.signOut(); return 'ok'; },
        );
        await tester.pumpAndSettle();
        expect(find.text(Copy.confirmItsYou), findsOneWidget);

        await tester.tap(find.text(Copy.cancel));
        await tester.pumpAndSettle();

        final result = await future;
        expect(result, isNull);
        expect(find.text(Copy.confirmItsYou), findsNothing);
        expect(find.text(Copy.stepUpNotConfirmed), findsOneWidget); // SnackBar
      },
    );

    testWidgets(
      's9_step_up_timeout gives up after five minutes',
      (tester) async {
        var now = DateTime.utc(2026, 1, 1, 12, 0, 0);
        final fakeApi = FakeApiClient();
        fakeApi.session = authenticated;
        final controller = StepUpController(
          api: fakeApi,
          session: SessionModel(api: fakeApi),
          browser: FakeBrowser(),
          now: () => now,
          pollEvery: const Duration(seconds: 2),
          giveUpAfter: const Duration(minutes: 5),
        );
        await tester.pumpWidget(buildApp(fakeApi, stepUp: controller));
        await tester.pumpAndSettle();

        fakeApi.nextError = stepUpRequired();
        final future = controller.run(
          waitingActionLabel: 'to disconnect jane@example.com',
          action: () async { await fakeApi.signOut(); return 'ok'; },
        );
        await tester.pumpAndSettle();

        await tester.tap(find.text(Copy.continueWithGoogle));
        await tester.pump();
        await tester.pump(const Duration(milliseconds: 50));

        // Advance the fake clock past the deadline.
        now = DateTime.utc(2026, 1, 1, 12, 6, 0);
        await tester.pump(const Duration(seconds: 2));
        await tester.pumpAndSettle();

        final result = await future;
        expect(result, isNull);
        expect(controller.status, StepUpStatus.idle);
        expect(find.text(Copy.stepUpNotConfirmed), findsOneWidget);
      },
    );

    testWidgets(
      's9_step_up_popup_blocked shows the pop-up message',
      (tester) async {
        final fakeApi = FakeApiClient();
        fakeApi.session = authenticated;
        final browser = FakeBrowser()..popupBlocked = true;
        final controller = StepUpController(
          api: fakeApi,
          session: SessionModel(api: fakeApi),
          browser: browser,
        );
        await tester.pumpWidget(buildApp(fakeApi, browser: browser, stepUp: controller));
        await tester.pumpAndSettle();

        fakeApi.nextError = stepUpRequired();
        final future = controller.run(
          waitingActionLabel: 'to disconnect jane@example.com',
          action: () async { await fakeApi.signOut(); return 'ok'; },
        );
        await tester.pumpAndSettle();

        await tester.tap(find.text(Copy.continueWithGoogle));
        await tester.pumpAndSettle();

        expect(controller.status, StepUpStatus.popupBlocked);
        expect(find.text(Copy.popupBlocked), findsOneWidget);
        // Button stays for a retry.
        final button = tester.widget<ElevatedButton>(
          find.byType(ElevatedButton),
        );
        expect(button.onPressed, isNotNull);

        controller.cancel();
        await tester.pumpAndSettle();
        await future;
      },
    );

    testWidgets(
      'step-up start uses intent step_up and no invite token',
      (tester) async {
        final fakeApi = FakeApiClient();
        fakeApi.session = authenticated;
        final browser = FakeBrowser();
        final controller = StepUpController(
          api: fakeApi,
          session: SessionModel(api: fakeApi),
          browser: browser,
        );
        await tester.pumpWidget(buildApp(fakeApi, browser: browser, stepUp: controller));
        await tester.pumpAndSettle();

        fakeApi.nextError = stepUpRequired();
        final future = controller.run(
          waitingActionLabel: 'to disconnect jane@example.com',
          action: () async { await fakeApi.signOut(); return 'ok'; },
        );
        await tester.pumpAndSettle();

        await tester.tap(find.text(Copy.continueWithGoogle));
        await tester.pump();
        await tester.pump(const Duration(milliseconds: 50));

        final startCall = fakeApi.calls.firstWhere(
          (c) => c.path == '/api/v1/auth/google/start',
        );
        final body = startCall.body as Map<String, Object?>;
        expect(body['intent'], equals('step_up'));
        expect(body['invite_token'], isNull);

        controller.cancel();
        await tester.pumpAndSettle();
        await future;
      },
    );

    test('step-up runs the action directly when no step-up is needed', () async {
      final fakeApi = FakeApiClient();
      fakeApi.session = authenticated;
      final controller = StepUpController(
        api: fakeApi,
        session: SessionModel(api: fakeApi),
        browser: FakeBrowser(),
      );
      var calls = 0;
      final result = await controller.run(
        waitingActionLabel: 'to disconnect jane@example.com',
        action: () async {
          calls++;
          return 'ok';
        },
      );
      expect(result, 'ok');
      expect(calls, 1);
      expect(controller.status, StepUpStatus.idle);
    });

    test('step-up does not loop when the retried action asks again', () async {
      final fakeApi = FakeApiClient();
      fakeApi.session = authenticated;
      final browser = FakeBrowser();
      final controller = StepUpController(
        api: fakeApi,
        session: SessionModel(api: fakeApi),
        browser: browser,
        pollEvery: const Duration(milliseconds: 10),
      );
      var calls = 0;
      final future = controller.run(
        waitingActionLabel: 'to disconnect jane@example.com',
        action: () async {
          calls++;
          if (calls == 1) {
            throw stepUpRequired();
          }
          throw stepUpRequired(); // retried action asks again
        },
      );
      // Let the first step-up start.
      await Future<void>.delayed(const Duration(milliseconds: 20));
      controller.continueWithGoogle();
      await Future<void>.delayed(const Duration(milliseconds: 20));
      // Confirm the step-up.
      fakeApi.session = Session(
        state: SessionState.authenticated,
        user: SessionUser(userId: 'u1', isAdmin: false),
        stepUpValidUntil: _afterNow(),
      );
      final result = await future;
      expect(result, isNull);
      expect(calls, 2); // original + one re-run, no loop
      expect(controller.status, StepUpStatus.idle);
    });

    test('step-up rethrows errors other than step_up_required', () async {
      final fakeApi = FakeApiClient();
      fakeApi.session = authenticated;
      final controller = StepUpController(
        api: fakeApi,
        session: SessionModel(api: fakeApi),
        browser: FakeBrowser(),
      );
      final other = ApiException(status: 500, code: 'server_error', requestId: 'r');
      fakeApi.nextError = other;
      await expectLater(
        controller.run(
          waitingActionLabel: 'to disconnect jane@example.com',
          action: () async { await fakeApi.signOut(); return 'ok'; },
        ),
        throwsA(isA<ApiException>()),
      );
    });

    testWidgets('XC-03 Confirm it\'s you controls are labelled', (tester) async {
      final fakeApi = FakeApiClient();
      fakeApi.session = authenticated;
      final controller = StepUpController(
        api: fakeApi,
        session: SessionModel(api: fakeApi),
        browser: FakeBrowser(),
      );
      await tester.pumpWidget(buildApp(fakeApi, stepUp: controller));
      await tester.pumpAndSettle();

      fakeApi.nextError = stepUpRequired();
      final future = controller.run(
        waitingActionLabel: 'to disconnect jane@example.com',
        action: () async { await fakeApi.signOut(); return 'ok'; },
      );
      await tester.pumpAndSettle();

      expect(
        find.byWidgetPredicate(
          (w) => w is Semantics && w.properties.label == Copy.continueWithGoogle,
        ),
        findsWidgets,
      );
      expect(
        find.byWidgetPredicate(
          (w) => w is Semantics && w.properties.label == Copy.cancel,
        ),
        findsWidgets,
      );

      controller.cancel();
      await tester.pumpAndSettle();
      await future;
    });
  });
}

DateTime _afterNow() => DateTime.now().toUtc().add(const Duration(minutes: 1));
