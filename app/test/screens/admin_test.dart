import 'package:app/api/fake_api_client.dart';
import 'package:app/api/models/admin.dart';
import 'package:app/copy.dart';
import 'package:app/format.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import '../support/settings_harness.dart';

Invite _invite(String id, String email, InviteStatus status) => Invite(
  inviteId: id,
  emailAddress: email,
  status: status,
  createdAt: DateTime.utc(2026, 10),
  expiresAt: DateTime.utc(2026, 10, 8),
  lastSentAt: DateTime.utc(2026, 10, 2),
);

InviteRequest _request(String id, String email) => InviteRequest(
  requestId: id,
  emailAddress: email,
  createdAt: DateTime.utc(2026, 10, 3, 14, 9),
);

AdminUser _user(String id, String email, {bool signedIn = true, int n = 2}) =>
    AdminUser(
      userId: id,
      emailAddress: email,
      createdAt: DateTime.utc(2026, 9),
      isAdmin: false,
      signedIn: signedIn,
      mailboxCount: n,
    );

void main() {
  Future<SettingsHarness> open(
    WidgetTester tester, {
    bool isAdmin = true,
    List<Invite> invites = const [],
    List<InviteRequest> requests = const [],
    List<AdminUser> users = const [],
  }) async {
    tester.view.physicalSize = const Size(800, 2400);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.reset);
    final h = SettingsHarness(isAdmin: isAdmin);
    h.api.invitePages.add(Paged(items: invites));
    h.api.inviteRequestPages.add(Paged(items: requests));
    h.api.userPages.add(Paged(items: users));
    await h.pump(tester);
    await h.openSettingsEntry(tester, Copy.admin);
    return h;
  }

  Future<void> tab(WidgetTester tester, String label) async {
    await tester.tap(find.widgetWithText(Tab, label));
    await tester.pumpAndSettle();
  }

  Iterable<FakeCall> writes(SettingsHarness h) => h.api.calls.where(
    (c) => c.method != 'GET' && c.path != '/api/v1/feed/next',
  );

  testWidgets('AU-01 AC1 admin invites by email', (tester) async {
    final h = await open(tester);
    await tester.enterText(find.byType(TextField), '  new@example.test ');
    await tester.tap(find.text(Copy.inviteButton));
    await tester.pumpAndSettle();
    final call = writes(h).single;
    expect(call.method, 'POST');
    expect(call.path, '/api/v1/admin/invites');
    expect((call.body as Map)['email_address'], 'new@example.test');
  });

  testWidgets('AU-01 AC2 re-send calls resend', (tester) async {
    final h = await open(
      tester,
      invites: [_invite('i1', 'a@example.test', InviteStatus.pending)],
    );
    expect(
      find.text('Pending, ${formatDate(DateTime.utc(2026, 10, 2))}'),
      findsOneWidget,
    );
    await tester.tap(find.text(Copy.resend));
    await tester.pumpAndSettle();
    expect(writes(h).single.path, '/api/v1/admin/invites/i1/resend');
  });

  testWidgets('AU-01 AC4 revoke calls revoke after confirming', (tester) async {
    final h = await open(
      tester,
      invites: [_invite('i1', 'a@example.test', InviteStatus.pending)],
    );
    await tester.tap(find.text(Copy.revoke));
    await tester.pumpAndSettle();
    expect(writes(h), isEmpty);
    await tester.tap(find.text(Copy.revoke).last);
    await tester.pumpAndSettle();
    final call = writes(h).single;
    expect(call.method, 'DELETE');
    expect(call.path, '/api/v1/admin/invites/i1');
  });

  testWidgets('AU-01 AC4 used and revoked invites have no actions', (
    tester,
  ) async {
    await open(
      tester,
      invites: [
        _invite('i1', 'a@example.test', InviteStatus.used),
        _invite('i2', 'b@example.test', InviteStatus.expired),
      ],
    );
    expect(find.text(Copy.revoke), findsNothing);
    expect(find.text(Copy.resend), findsOneWidget);
  });

  testWidgets(
    'AU-01 AC5 an admin write without a fresh sign-in shows Confirm it\'s you',
    (tester) async {
      final h = await open(tester);
      h.api.nextAdminWriteError = problem(403, 'step_up_required');
      await tester.enterText(find.byType(TextField), 'new@example.test');
      await tester.tap(find.text(Copy.inviteButton));
      await tester.pumpAndSettle();
      expect(find.text(Copy.confirmItsYou), findsOneWidget);
      expect(find.text(Copy.stepUpInvite('new@example.test')), findsOneWidget);
      h.stepUp.cancel();
      await tester.pumpAndSettle();
    },
  );

  testWidgets('AU-02 AC2 requests list shows address and time with approve '
      'and decline', (tester) async {
    await open(tester, requests: [_request('r1', 'req@example.test')]);
    await tab(tester, Copy.adminRequests);
    expect(find.text('req@example.test'), findsOneWidget);
    expect(
      find.text(formatDateTime(DateTime.utc(2026, 10, 3, 14, 9))),
      findsOneWidget,
    );
    expect(find.text(Copy.approve), findsOneWidget);
    expect(find.text(Copy.decline), findsOneWidget);
  });

  testWidgets('AU-02 AC3 approve and decline call their endpoints', (
    tester,
  ) async {
    final h = await open(
      tester,
      requests: [
        _request('r1', 'one@example.test'),
        _request('r2', 'two@example.test'),
      ],
    );
    await tab(tester, Copy.adminRequests);
    await tester.tap(find.text(Copy.approve).first);
    await tester.pumpAndSettle();
    expect(writes(h).last.path, '/api/v1/admin/invite-requests/r1/approve');
    expect(find.text('one@example.test'), findsNothing);
    await tester.tap(find.text(Copy.decline));
    await tester.pumpAndSettle();
    await tester.tap(find.text(Copy.decline).last);
    await tester.pumpAndSettle();
    expect(writes(h).last.path, '/api/v1/admin/invite-requests/r2/decline');
    expect(find.text('two@example.test'), findsNothing);
  });

  testWidgets('AU-07 AC5 End session confirms then calls the admin endpoint', (
    tester,
  ) async {
    final h = await open(
      tester,
      users: [
        _user('u9', 'u@example.test', n: 1),
        _user('u8', 'off@example.test', signedIn: false),
      ],
    );
    await tab(tester, Copy.adminUsers);
    expect(find.text('1 mailbox'), findsOneWidget);
    expect(find.text('2 mailboxes'), findsOneWidget);
    final off = tester.widget<TextButton>(
      find.widgetWithText(TextButton, Copy.endSession).last,
    );
    expect(off.onPressed, isNull);
    await tester.tap(find.text(Copy.endSession).first);
    await tester.pumpAndSettle();
    expect(
      find.text(Copy.endSessionQuestion('u@example.test')),
      findsOneWidget,
    );
    expect(writes(h), isEmpty);
    await tester.tap(find.text(Copy.endSession).last);
    await tester.pumpAndSettle();
    final call = writes(h).single;
    expect(call.method, 'DELETE');
    expect(call.path, '/api/v1/admin/users/u9/sessions');
  });

  testWidgets('AU-01 lists page with load more', (tester) async {
    tester.view.physicalSize = const Size(800, 2400);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.reset);
    final h = SettingsHarness(isAdmin: true);
    h.api.invitePages.addAll([
      Paged(
        items: [_invite('i1', 'a@example.test', InviteStatus.used)],
        nextCursor: 'c2',
      ),
      Paged(items: [_invite('i2', 'b@example.test', InviteStatus.used)]),
    ]);
    await h.pump(tester);
    await h.openSettingsEntry(tester, Copy.admin);
    await tester.tap(find.text(Copy.loadMore));
    await tester.pumpAndSettle();
    expect(find.text('b@example.test'), findsOneWidget);
    expect(find.text('a@example.test'), findsOneWidget);
    expect(find.text(Copy.loadMore), findsNothing);
  });

  testWidgets('s9_admin_hidden_for_non_admins', (tester) async {
    final h = SettingsHarness();
    await h.pump(tester);
    await h.tapTab(tester, Copy.tabSettings);
    expect(find.text(Copy.admin), findsNothing);
    h.api.calls.clear();
    h.session.addListener(() {});
    // Reaching the route directly shows Admins only and calls nothing.
    final nav = tester.state<NavigatorState>(find.byType(Navigator).first);
    nav.pushNamed('/settings/admin');
    await tester.pumpAndSettle();
    expect(find.text(Copy.adminsOnly), findsOneWidget);
    expect(h.api.calls.where((c) => c.path.contains('/admin/')), isEmpty);
  });

  testWidgets('s9_admin_invalid_email is refused before any call', (
    tester,
  ) async {
    final h = await open(tester);
    for (final bad in ['nope', 'a@', '@b', 'a@b@c']) {
      await tester.enterText(find.byType(TextField), bad);
      await tester.tap(find.text(Copy.inviteButton));
      await tester.pumpAndSettle();
      expect(find.text(Copy.emailInvalid), findsOneWidget);
    }
    expect(writes(h), isEmpty);
  });

  testWidgets('XC-03 Experiments and Admin controls are labelled (admin)', (
    tester,
  ) async {
    final handle = tester.ensureSemantics();
    await open(
      tester,
      invites: [_invite('i1', 'a@example.test', InviteStatus.pending)],
    );
    expect(find.bySemanticsLabel(Copy.emailAddressLabel), findsOneWidget);
    expect(find.bySemanticsLabel(Copy.inviteButton), findsOneWidget);
    expect(find.bySemanticsLabel(Copy.resend), findsOneWidget);
    expect(find.bySemanticsLabel(Copy.revoke), findsOneWidget);
    handle.dispose();
  });

  testWidgets('AU-01 a 403 forbidden write shows the failure message', (
    tester,
  ) async {
    final h = await open(tester);
    h.api.nextAdminWriteError = problem(403, 'forbidden');
    await tester.enterText(find.byType(TextField), 'new@example.test');
    await tester.tap(find.text(Copy.inviteButton));
    await tester.pumpAndSettle();
    expect(find.text(Copy.actionFailed), findsOneWidget);
  });
}
