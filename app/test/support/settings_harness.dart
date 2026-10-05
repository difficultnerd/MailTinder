import 'package:app/api/api_client.dart';
import 'package:app/api/fake_api_client.dart';
import 'package:app/api/models/session.dart';
import 'package:app/app.dart';
import 'package:app/copy.dart';
import 'package:app/state/session_model.dart';
import 'package:app/state/sign_in_model.dart';
import 'package:app/state/step_up_controller.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import 'fake_browser.dart';

Session authenticatedSession({bool isAdmin = false}) => Session(
  state: SessionState.authenticated,
  user: SessionUser(userId: 'u1', isAdmin: isAdmin),
);

Mailbox mailbox(
  String id,
  String address, {
  MailboxStatus status = MailboxStatus.connected,
}) => Mailbox(
  mailboxId: id,
  provider: 'gmail',
  emailAddress: address,
  status: status,
);

ApiException problem(int status, String code) =>
    ApiException(status: status, code: code, requestId: 'r1');

class SettingsHarness {
  SettingsHarness({bool isAdmin = false}) {
    api.session = authenticatedSession(isAdmin: isAdmin);
    session = SessionModel(api: api);
    stepUp = StepUpController(api: api, session: session, browser: browser);
  }

  final FakeApiClient api = FakeApiClient();
  final FakeBrowser browser = FakeBrowser();
  late final SessionModel session;
  late final StepUpController stepUp;

  Future<void> pump(WidgetTester tester) async {
    await tester.pumpWidget(
      MailTinderApp(
        session: session,
        api: api,
        signInModel: SignInModel(api: api, browser: browser),
        browser: browser,
        stepUp: stepUp,
      ),
    );
    await tester.pumpAndSettle();
  }

  Future<void> tapTab(WidgetTester tester, String label) async {
    await tester.tap(
      find.descendant(
        of: find.byType(NavigationBar),
        matching: find.text(label),
      ),
    );
    await tester.pumpAndSettle();
  }

  /// Opens Settings, then the entry titled [entry].
  Future<void> openSettingsEntry(WidgetTester tester, String entry) async {
    await tapTab(tester, Copy.tabSettings);
    await tester.tap(find.text(entry));
    await tester.pumpAndSettle();
  }
}
