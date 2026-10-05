import 'package:app/api/models/auth.dart';
import 'package:app/api/models/session.dart';
import 'package:app/copy.dart';
import 'package:app/routes.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import '../support/settings_harness.dart';

void main() {
  Future<SettingsHarness> open(
    WidgetTester tester,
    List<Mailbox> boxes, {
    bool isAdmin = false,
  }) async {
    final h = SettingsHarness(isAdmin: isAdmin);
    h.api.mailboxes.addAll(boxes);
    await h.pump(tester);
    await h.openSettingsEntry(tester, Copy.settingsConnectedAccounts);
    return h;
  }

  testWidgets(
    'ST-03 AC1 Connected accounts lists provider, address and status',
    (tester) async {
      await open(tester, [mailbox('m1', 'a@example.com')]);
      expect(find.text('Gmail'), findsOneWidget);
      expect(find.text('a@example.com'), findsOneWidget);
      expect(find.text(Copy.statusConnected), findsOneWidget);
    },
  );

  testWidgets(
    'AU-04 AC1 Add Gmail starts the link intent and navigates to Google',
    (tester) async {
      final h = await open(tester, [mailbox('m1', 'a@example.com')]);
      await tester.tap(find.text(Copy.addGmail));
      await tester.pumpAndSettle();
      final start = h.api.calls.where(
        (c) => c.path.endsWith('auth/google/start'),
      );
      expect((start.single.body! as Map)['intent'], 'link');
      expect(h.browser.assigned.single, h.api.authUrl);
    },
  );

  testWidgets('AU-04 AC2 three Gmail mailboxes are listed by address', (
    tester,
  ) async {
    await open(tester, [
      mailbox('m1', 'a@example.com'),
      mailbox('m2', 'b@example.com'),
      mailbox('m3', 'c@example.com'),
    ]);
    expect(find.text('Gmail'), findsNWidgets(3));
    for (final a in ['a', 'b', 'c']) {
      expect(find.text('$a@example.com'), findsOneWidget);
    }
  });

  testWidgets(
    "AU-04 AC6 Add Gmail without a fresh sign-in shows Confirm it's you",
    (tester) async {
      final h = await open(tester, [mailbox('m1', 'a@example.com')]);
      h.api.nextError = problem(403, 'step_up_required');
      await tester.tap(find.text(Copy.addGmail));
      await tester.pumpAndSettle();
      expect(find.text(Copy.confirmItsYou), findsOneWidget);
      expect(find.text(Copy.stepUpAddGmail), findsOneWidget);
      h.stepUp.cancel();
      await tester.pumpAndSettle();
    },
  );

  testWidgets(
    'AU-05 AC2 the only mailbox points to delete account and sends nothing',
    (tester) async {
      final h = await open(tester, [mailbox('m1', 'a@example.com')]);
      await tester.tap(find.text(Copy.disconnect));
      await tester.pumpAndSettle();
      expect(find.text(Copy.onlyMailbox), findsOneWidget);
      expect(h.api.calls.any((c) => c.method == 'DELETE'), isFalse);
      await tester.tap(find.text(Copy.goToAccount));
      await tester.pumpAndSettle();
      expect(find.text(Copy.deleteAccount), findsOneWidget);
    },
  );

  testWidgets(
    "AU-05 AC3 Disconnect without a fresh sign-in shows Confirm it's you",
    (tester) async {
      final h = await open(tester, [
        mailbox('m1', 'a@example.com'),
        mailbox('m2', 'b@example.com'),
      ]);
      h.api.nextDisconnectError = problem(403, 'step_up_required');
      await tester.tap(find.text(Copy.disconnect).last);
      await tester.pumpAndSettle();
      expect(
        find.text(Copy.disconnectQuestion('b@example.com')),
        findsOneWidget,
      );
      await tester.tap(find.text(Copy.disconnect).last);
      await tester.pumpAndSettle();
      expect(find.text(Copy.confirmItsYou), findsOneWidget);
      expect(find.text(Copy.stepUpDisconnect('b@example.com')), findsOneWidget);
      h.stepUp.cancel();
      await tester.pumpAndSettle();
    },
  );

  testWidgets(
    'AU-05 AC4 app_folder_move_failed shows the S9 message and keeps the mailbox',
    (tester) async {
      final h = await open(tester, [
        mailbox('m1', 'a@example.com'),
        mailbox('m2', 'b@example.com'),
      ]);
      h.api.nextDisconnectError = problem(409, 'app_folder_move_failed');
      await tester.tap(find.text(Copy.disconnect).last);
      await tester.pumpAndSettle();
      await tester.tap(find.text(Copy.disconnect).last);
      await tester.pumpAndSettle();
      expect(find.text(Copy.appFolderMoveFailed), findsOneWidget);
      expect(find.text('b@example.com'), findsOneWidget);
    },
  );

  testWidgets(
    'asvs_v10_7_3 Settings lists linked mailboxes and Disconnect calls the API',
    (tester) async {
      final h = await open(tester, [
        mailbox('m1', 'a@example.com'),
        mailbox('m2', 'b@example.com'),
      ]);
      await tester.tap(find.text(Copy.disconnect).last);
      await tester.pumpAndSettle();
      await tester.tap(find.text(Copy.disconnect).last);
      await tester.pumpAndSettle();
      expect(
        h.api.calls.any((c) => c.method == 'DELETE' && c.path.endsWith('/m2')),
        isTrue,
      );
      expect(find.text('b@example.com'), findsNothing);
      expect(find.text('a@example.com'), findsOneWidget);
    },
  );

  testWidgets('s9_connected_accounts_needs_sign_in offers Sign in again', (
    tester,
  ) async {
    final h = await open(tester, [
      mailbox('m1', 'a@example.com', status: MailboxStatus.needsSignIn),
      mailbox('m2', 'b@example.com'),
    ]);
    expect(find.text(Copy.statusNeedsSignIn), findsOneWidget);
    await tester.tap(find.text(Copy.signInAgain));
    await tester.pumpAndSettle();
    final start = h.api.calls.where(
      (c) => c.path.endsWith('auth/google/start'),
    );
    final body = start.single.body! as Map;
    expect(body['intent'], 'reconnect');
    expect(body['mailbox_id'], 'm1');
    expect(h.browser.assigned, isNotEmpty);
  });

  testWidgets('s9_connected_accounts_linked_elsewhere shows the message', (
    tester,
  ) async {
    final h = SettingsHarness();
    h.api.mailboxes.add(mailbox('m1', 'a@example.com'));
    await h.pump(tester);
    final nav = tester.state<NavigatorState>(find.byType(Navigator));
    nav.pushNamed(
      Routes.settingsAccounts,
      arguments: AuthOutcome.mailboxLinkedElsewhere,
    );
    await tester.pumpAndSettle();
    expect(find.text(Copy.mailboxLinkedElsewhere), findsOneWidget);
  });

  testWidgets('s9_settings_admin_entries hidden for non-admins', (
    tester,
  ) async {
    final h = SettingsHarness();
    await h.pump(tester);
    await h.tapTab(tester, Copy.tabSettings);
    expect(find.text(Copy.settingsConnectedAccounts), findsOneWidget);
    expect(find.text(Copy.settingsAccount), findsOneWidget);
  });
}
