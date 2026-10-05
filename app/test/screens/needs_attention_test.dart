import 'dart:async';

import 'package:app/api/api_client.dart';
import 'package:app/api/fake_api_client.dart';
import 'package:app/api/models/auth.dart';
import 'package:app/api/models/needs_attention.dart';
import 'package:app/api/models/session.dart';
import 'package:app/copy.dart';
import 'package:app/screens/home/home_shell.dart';
import 'package:app/screens/needs_attention/needs_attention_screen.dart';
import 'package:app/state/needs_attention_model.dart';
import 'package:app/state/session_model.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import '../support/fake_browser.dart';

const _jane = Mailbox(
  mailboxId: 'mb-1',
  provider: 'gmail',
  emailAddress: 'jane@example.com',
  status: MailboxStatus.connected,
);
const _bobSignedOut = Mailbox(
  mailboxId: 'mb-2',
  provider: 'gmail',
  emailAddress: 'bob@example.com',
  status: MailboxStatus.needsSignIn,
);

NeedsAttentionItem _item({
  String itemId = 'na-1',
  String mailboxId = 'mb-1',
  String senderDisplay = 'Acme News',
  NaReason reason = NaReason.httpsOnlyUnsubscribe,
  String? link = 'https://unsub.example.com/acme',
  DateTime? createdAt,
}) {
  return NeedsAttentionItem(
    itemId: itemId,
    mailboxId: mailboxId,
    senderDisplay: senderDisplay,
    reason: reason,
    link: link == null ? null : Uri.parse(link),
    createdAt: createdAt ?? DateTime.utc(2026, 10, 1, 9),
  );
}

class _Harness {
  _Harness() {
    api = FakeApiClient();
    api.session = const Session(
      state: SessionState.authenticated,
      user: SessionUser(userId: 'u1', isAdmin: false),
      mailboxes: [_jane, _bobSignedOut],
    );
    browser = FakeBrowser();
    session = SessionModel(api: api);
    model = NeedsAttentionModel(api: api);
  }

  late final FakeApiClient api;
  late final FakeBrowser browser;
  late final SessionModel session;
  late final NeedsAttentionModel model;
}

/// Builds a harness, seeds one Needs Attention page and refreshes the session
/// so `session.mailboxes` is available for the mailbox badges.
Future<_Harness> _make({
  List<NeedsAttentionItem> items = const [],
  int? openCount,
  Completer<void>? gate,
}) async {
  final h = _Harness();
  h.api.needsAttentionPages.add(
    NeedsAttentionPage(
      items: items,
      openCount: openCount ?? items.length,
      nextCursor: null,
    ),
  );
  h.api.needsAttentionGate = gate;
  await h.session.refresh();
  return h;
}

Widget _screen(_Harness h) {
  return MaterialApp(
    home: Scaffold(
      body: NeedsAttentionScreen(
        model: h.model,
        session: h.session,
        api: h.api,
        browser: h.browser,
      ),
    ),
  );
}

void main() {
  testWidgets('NA-01 AC1 lists items newest first with sender, mailbox badge, '
      'reason and actions', (tester) async {
    final h = await _make(
      items: [
        _item(
          itemId: 'na-old',
          senderDisplay: 'Older Sender',
          createdAt: DateTime.utc(2026, 1, 1, 9),
        ),
        _item(
          itemId: 'na-new',
          senderDisplay: 'Newer Sender',
          createdAt: DateTime.utc(2026, 6, 1, 9),
        ),
      ],
    );
    await tester.pumpWidget(_screen(h));
    await tester.pumpAndSettle();

    expect(find.text('Newer Sender'), findsOneWidget);
    expect(find.text('Older Sender'), findsOneWidget);
    expect(
      tester.getTopLeft(find.text('Newer Sender')).dy,
      lessThan(tester.getTopLeft(find.text('Older Sender')).dy),
    );

    // Mailbox badge and reason copy.
    expect(find.text('jane@example.com'), findsWidgets);
    expect(find.text(Copy.naHttpsOnlyUnsubscribe), findsWidgets);

    // The three actions.
    expect(find.text(Copy.openUnsubscribePage), findsWidgets);
    expect(find.text(Copy.done), findsWidgets);
    expect(find.text(Copy.dismiss), findsWidgets);
  });

  testWidgets('NA-01 AC2 Done resolves and Dismiss dismisses', (tester) async {
    final h = await _make(
      items: [
        _item(
          itemId: 'na-1',
          senderDisplay: 'Acme One',
          createdAt: DateTime.utc(2026, 6, 1, 9),
        ),
        _item(
          itemId: 'na-2',
          senderDisplay: 'Acme Two',
          createdAt: DateTime.utc(2026, 1, 1, 9),
        ),
      ],
    );
    await tester.pumpWidget(_screen(h));
    await tester.pumpAndSettle();

    expect(h.model.openCount, 2);

    await tester.tap(find.text(Copy.done).first);
    await tester.pumpAndSettle();

    expect(h.api.lastResolvedItemId, 'na-1');
    expect(find.text('Acme One'), findsNothing);
    expect(h.model.openCount, 1);

    await tester.tap(find.text(Copy.dismiss).first);
    await tester.pumpAndSettle();

    expect(h.api.lastDismissedItemId, 'na-2');
    expect(find.text('Acme Two'), findsNothing);
    expect(h.model.openCount, 0);
  });

  testWidgets('UN-01 AC6 signed-out mailbox item offers Sign in again', (
    tester,
  ) async {
    final h = await _make(
      items: [
        _item(
          mailboxId: 'mb-2',
          senderDisplay: 'Bob Weekly',
          reason: NaReason.mailboxNeedsSignIn,
        ),
      ],
    );
    await tester.pumpWidget(_screen(h));
    await tester.pumpAndSettle();

    expect(
      find.text(Copy.naMailboxNeedsSignIn('Bob Weekly', 'bob@example.com')),
      findsOneWidget,
    );
    expect(find.text(Copy.signInAgain), findsWidgets);
    expect(find.text(Copy.openUnsubscribePage), findsNothing);

    await tester.tap(find.text(Copy.signInAgain).first);
    await tester.pumpAndSettle();

    final authCalls = h.api.calls.where(
      (call) => call.path == '/api/v1/auth/google/start',
    );
    expect(authCalls, isNotEmpty);
    expect(
      (authCalls.first.body! as Map<String, Object?>)['intent'],
      AuthIntent.reconnect.wire,
    );
    expect(
      (authCalls.first.body! as Map<String, Object?>)['mailbox_id'],
      'mb-2',
    );
    expect(h.browser.assigned, [h.api.authUrl]);
  });

  testWidgets('UN-04 AC6 Open unsubscribe page opens the item link only', (
    tester,
  ) async {
    final h = await _make(
      items: [_item(link: 'https://unsub.example.com/acme')],
    );
    await tester.pumpWidget(_screen(h));
    await tester.pumpAndSettle();

    await tester.tap(find.text(Copy.openUnsubscribePage).first);
    await tester.pumpAndSettle();

    expect(h.browser.openedExternal, [
      Uri.parse('https://unsub.example.com/acme'),
    ]);
    expect(h.browser.assigned, isEmpty);
  });

  testWidgets('UN-05 AC1 failed job item offers Open unsubscribe page', (
    tester,
  ) async {
    final h = await _make(
      items: [
        _item(
          senderDisplay: 'Gadget Store',
          reason: NaReason.jobExpired,
          link: 'https://unsub.example.com/gadgets',
        ),
      ],
    );
    await tester.pumpWidget(_screen(h));
    await tester.pumpAndSettle();

    expect(find.text('Gadget Store'), findsOneWidget);
    expect(find.text(Copy.naJobExpired), findsOneWidget);
    expect(find.text(Copy.openUnsubscribePage), findsOneWidget);
  });

  testWidgets("XC-04 failed Done puts the item back with Couldn't do that", (
    tester,
  ) async {
    final h = await _make(items: [_item(senderDisplay: 'Acme News')]);
    h.api.nextResolveError = const NetworkException();
    await tester.pumpWidget(_screen(h));
    await tester.pumpAndSettle();

    await tester.tap(find.text(Copy.done).first);
    await tester.pumpAndSettle();

    expect(find.text('Acme News'), findsOneWidget);
    expect(find.text(Copy.actionFailed), findsOneWidget);
    expect(h.model.openCount, 1);
  });

  testWidgets('s9_needs_attention_empty shows Nothing needs you', (
    tester,
  ) async {
    final h = await _make();
    await tester.pumpWidget(_screen(h));
    await tester.pumpAndSettle();

    expect(find.text(Copy.nothingNeedsYou), findsOneWidget);
  });

  testWidgets('s9_needs_attention_loading shows a progress indicator', (
    tester,
  ) async {
    final gate = Completer<void>();
    final h = await _make(items: [_item()], gate: gate);
    await tester.pumpWidget(_screen(h));
    await tester.pump();

    expect(find.byType(CircularProgressIndicator), findsOneWidget);

    gate.complete();
    await tester.pumpAndSettle();
    expect(find.text('Acme News'), findsOneWidget);
  });

  testWidgets('s9_needs_attention_badge shows open_count', (tester) async {
    final h = await _make(items: [], openCount: 3);
    await tester.pumpWidget(
      MaterialApp(
        home: HomeShell(
          needsAttention: h.model,
          session: h.session,
          api: h.api,
          browser: h.browser,
        ),
      ),
    );
    await tester.pumpAndSettle();

    expect(h.model.openCount, 3);
    expect(
      find.descendant(of: find.byType(Badge), matching: find.text('3')),
      findsWidgets,
    );
  });

  testWidgets('s9_needs_attention_reason_copy matches S9 for each v1 reason', (
    tester,
  ) async {
    final cases = <NaReason, (String sender, String mailboxId, String text)>{
      NaReason.httpsOnlyUnsubscribe: (
        'Acme News',
        'mb-1',
        Copy.naHttpsOnlyUnsubscribe,
      ),
      NaReason.oneClickRedirect: ('Acme News', 'mb-1', Copy.naOneClickRedirect),
      NaReason.oneClickAddressRefused: (
        'Acme News',
        'mb-1',
        Copy.naOneClickAddressRefused,
      ),
      NaReason.unsubscribeIgnored: (
        'Acme News',
        'mb-1',
        Copy.naUnsubscribeIgnored,
      ),
      NaReason.unsubscribeFailed: (
        'Acme News',
        'mb-1',
        Copy.naUnsubscribeFailed,
      ),
      NaReason.jobExpired: ('Acme News', 'mb-1', Copy.naJobExpired),
      NaReason.mailboxNeedsSignIn: (
        'Bob Weekly',
        'mb-2',
        Copy.naMailboxNeedsSignIn('Bob Weekly', 'bob@example.com'),
      ),
      NaReason.other: ('Acme News', 'mb-1', Copy.naOther),
    };

    for (final entry in cases.entries) {
      final (sender, mailboxId, text) = entry.value;
      final h = await _make(
        items: [
          _item(reason: entry.key, senderDisplay: sender, mailboxId: mailboxId),
        ],
      );
      await tester.pumpWidget(_screen(h));
      await tester.pumpAndSettle();

      expect(find.text(text), findsOneWidget, reason: '${entry.key}');
    }
  });

  testWidgets('a javascript or http link is never opened', (tester) async {
    for (final link in ['javascript:alert(1)', 'http://unsub.example.com/x']) {
      final h = await _make(items: [_item(link: link)]);
      await tester.pumpWidget(_screen(h));
      await tester.pumpAndSettle();

      final button = find.text(Copy.openUnsubscribePage);
      expect(button, findsOneWidget, reason: link);
      await tester.tap(button);
      await tester.pumpAndSettle();

      expect(h.browser.openedExternal, isEmpty, reason: link);
    }
  });

  testWidgets('XC-03 Needs Attention controls are labelled', (tester) async {
    final handle = tester.ensureSemantics();
    final h = await _make(items: [_item()]);
    await tester.pumpWidget(_screen(h));
    await tester.pumpAndSettle();

    expect(find.bySemanticsLabel(Copy.openUnsubscribePage), findsWidgets);
    expect(find.bySemanticsLabel(Copy.done), findsWidgets);
    expect(find.bySemanticsLabel(Copy.dismiss), findsWidgets);

    handle.dispose();
  });
}
