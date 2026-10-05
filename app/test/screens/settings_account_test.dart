import 'package:app/api/api_client.dart';
import 'package:app/copy.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import '../support/settings_harness.dart';

void main() {
  Future<SettingsHarness> open(WidgetTester tester) async {
    final h = SettingsHarness();
    await h.pump(tester);
    await h.openSettingsEntry(tester, Copy.settingsAccount);
    return h;
  }

  Future<void> confirmDelete(WidgetTester tester) async {
    await tester.tap(find.text(Copy.deleteAccount));
    await tester.pumpAndSettle();
    await tester.tap(find.text(Copy.continueButton));
    await tester.pumpAndSettle();
    await tester.tap(find.text(Copy.deleteAccount).last);
    await tester.pumpAndSettle();
  }

  testWidgets('AU-07 AC2 Sign out ends the session and returns to Sign-in', (
    tester,
  ) async {
    final h = await open(tester);
    await tester.tap(find.text(Copy.signOut));
    await tester.pumpAndSettle();
    expect(h.api.calls.any((c) => c.path.endsWith('auth/sign-out')), isTrue);
    expect(h.session.session, isNull);
    expect(find.text(Copy.continueWithGoogle), findsOneWidget);
  });

  testWidgets('GM-02 AC2 Sounds switch is off by default', (tester) async {
    await open(tester);
    final tile = tester.widget<SwitchListTile>(find.byType(SwitchListTile));
    expect(tile.value, isFalse);
    expect(find.text(Copy.sounds), findsOneWidget);
    await tester.tap(find.byType(Switch));
    await tester.pump();
    expect(
      tester.widget<SwitchListTile>(find.byType(SwitchListTile)).value,
      isTrue,
    );
  });

  testWidgets('asvs_v7_4_4 sign out is reachable from every tab', (
    tester,
  ) async {
    final h = SettingsHarness();
    await h.pump(tester);
    for (final tab in [Copy.tabFeed, Copy.tabFiled, Copy.tabNeedsAttention]) {
      await h.tapTab(tester, tab);
      await h.openSettingsEntry(tester, Copy.settingsAccount);
      expect(find.text(Copy.signOut), findsOneWidget);
      await tester.pageBack();
      await tester.pumpAndSettle();
    }
  });

  testWidgets('asvs_v7_5_2 Account shows Sign out for the one session', (
    tester,
  ) async {
    await open(tester);
    expect(find.text(Copy.signOut), findsOneWidget);
  });

  testWidgets(
    'asvs_v14_3_1 sign out wipes memory when the server cannot be reached',
    (tester) async {
      final h = await open(tester);
      var wiped = false;
      h.session.addWipeListener(() => wiped = true);
      h.api.nextError = const NetworkException();
      await tester.tap(find.text(Copy.signOut));
      await tester.pumpAndSettle();
      expect(wiped, isTrue);
      expect(h.session.session, isNull);
      expect(find.text(Copy.continueWithGoogle), findsOneWidget);
    },
  );

  testWidgets(
    "AU-06 AC3 delete account without a fresh sign-in shows Confirm it's you",
    (tester) async {
      final h = await open(tester);
      h.api.nextDeleteError = problem(403, 'step_up_required');
      await confirmDelete(tester);
      expect(find.text(Copy.confirmItsYou), findsOneWidget);
      expect(find.text(Copy.stepUpDeleteAccount), findsOneWidget);
      h.stepUp.cancel();
      await tester.pumpAndSettle();
    },
  );

  testWidgets('s9_account_delete two-step confirm explains labels stay', (
    tester,
  ) async {
    final h = await open(tester);
    await tester.tap(find.text(Copy.deleteAccount));
    await tester.pumpAndSettle();
    expect(find.text(Copy.deleteAccountExplain), findsOneWidget);
    expect(
      Copy.deleteAccountExplain,
      contains('Labels already on your messages stay'),
    );
    await tester.tap(find.text(Copy.continueButton));
    await tester.pumpAndSettle();
    expect(find.text(Copy.deleteAccountConfirm), findsOneWidget);
    expect(h.api.calls.any((c) => c.path.endsWith('/account')), isFalse);
    await tester.tap(find.text(Copy.deleteAccount).last);
    await tester.pumpAndSettle();
    expect(h.api.calls.any((c) => c.path.endsWith('/account')), isTrue);
    expect(find.text(Copy.accountDeleted), findsOneWidget);
    expect(find.text(Copy.continueWithGoogle), findsOneWidget);
  });

  testWidgets('s9_account_delete app folders not deleted lists the mailboxes', (
    tester,
  ) async {
    final h = await open(tester);
    h.api.deleteResult = DeleteAccountResult(
      deletionDueBy: DateTime.utc(2026, 10, 6),
      appFoldersNotDeleted: const [
        (mailboxId: 'm1', emailAddress: 'a@example.com'),
      ],
    );
    await confirmDelete(tester);
    expect(find.text(Copy.appFoldersNotDeleted), findsOneWidget);
    expect(find.text('a@example.com'), findsOneWidget);
    await tester.tap(find.text(Copy.close));
    await tester.pumpAndSettle();
    expect(find.text(Copy.accountDeleted), findsOneWidget);
  });
}
