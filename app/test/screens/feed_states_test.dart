import 'dart:async';

import 'package:app/api/api_client.dart';
import 'package:app/api/fake_api_client.dart';
import 'package:app/api/models/feed.dart';
import 'package:app/api/models/session.dart';
import 'package:app/copy.dart';
import 'package:app/screens/feed/feed_screen.dart';
import 'package:app/state/feed_model.dart';
import 'package:app/state/session_model.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import '../support/cards.dart';
import '../support/fake_browser.dart';
import '../support/fake_connectivity.dart';

const _jane = Mailbox(
  mailboxId: 'mb-1',
  provider: 'gmail',
  emailAddress: 'jane@example.com',
  status: MailboxStatus.connected,
);
const _bob = Mailbox(
  mailboxId: 'mb-2',
  provider: 'gmail',
  emailAddress: 'bob@example.com',
  status: MailboxStatus.connected,
);

class _Harness {
  _Harness({List<Mailbox> mailboxes = const [], bool online = true}) {
    api = FakeApiClient();
    api.session = Session(
      state: SessionState.authenticated,
      user: const SessionUser(userId: 'u1', isAdmin: false),
      mailboxes: mailboxes,
    );
    browser = FakeBrowser();
    connectivity = FakeConnectivity(online: online);
    session = SessionModel(api: api);
    model = FeedModel(api: api, session: session, connectivity: connectivity);
  }

  late final FakeApiClient api;
  late final FakeBrowser browser;
  late final FakeConnectivity connectivity;
  late final SessionModel session;
  late final FeedModel model;
}

Widget _app(_Harness h) {
  return MaterialApp(
    home: Scaffold(
      body: FeedScreen(
        model: h.model,
        session: h.session,
        api: h.api,
        browser: h.browser,
      ),
    ),
  );
}

/// Builds a harness and refreshes the session so [SessionModel.session]
/// reflects [FakeApiClient.session] (addresses, mailboxes).
Future<_Harness> _make({
  List<Mailbox> mailboxes = const [],
  bool online = true,
}) async {
  final h = _Harness(mailboxes: mailboxes, online: online);
  await h.session.refresh();
  return h;
}

void main() {
  testWidgets('FD-01 AC1 one card in focus shows every card field', (
    tester,
  ) async {
    final h = await _make(mailboxes: const [_jane]);
    h.api.feedPages.addAll([
      pageOf([
        buildCard(
          senderName: 'Jane Sender',
          senderAddress: 'sender@example.com',
          subject: 'Weekly roundup',
          preview: 'Here is the preview text.',
          bulkScore: 90,
          bulkReason: 'Sent from a mailing list.',
          messageClass: MessageClass.list,
        ),
      ]),
    ]);
    await tester.pumpWidget(_app(h));
    await tester.pumpAndSettle();

    expect(find.text('Jane Sender'), findsOneWidget);
    expect(find.text('sender@example.com'), findsOneWidget);
    expect(find.text('Weekly roundup'), findsOneWidget);
    expect(find.text('Here is the preview text.'), findsOneWidget);
    expect(find.text('jane@example.com'), findsOneWidget); // mailbox badge
    expect(find.text(Copy.bulkBadge(90)), findsWidgets);
  });

  testWidgets('FD-01 AC2 preview renders as plain text', (tester) async {
    final h = await _make();
    h.api.feedPages.addAll([
      pageOf([buildCard(preview: '<b>x</b> example.com')]),
    ]);
    await tester.pumpWidget(_app(h));
    await tester.pumpAndSettle();

    expect(find.text('<b>x</b> example.com'), findsOneWidget);
  });

  testWidgets('FD-02 AC2 each card shows its mailbox address', (tester) async {
    final h = await _make(mailboxes: const [_jane, _bob]);
    h.api.feedPages.addAll([
      pageOf([
        buildCard(messageId: 'a', mailboxId: 'mb-1'),
        buildCard(messageId: 'b', mailboxId: 'mb-2'),
      ]),
    ]);
    await tester.pumpWidget(_app(h));
    await tester.pumpAndSettle();

    expect(find.text('jane@example.com'), findsOneWidget);
    expect(find.text('bob@example.com'), findsOneWidget);
  });

  testWidgets(
    'FD-02 AC3 one failing mailbox shows a banner naming it and other cards still load',
    (tester) async {
      final h = await _make(mailboxes: const [_jane, _bob]);
      h.api.feedPages.addAll([
        pageOf(
          [buildCard(messageId: 'a', mailboxId: 'mb-1')],
          mailboxErrors: [signInError('mb-2')],
        ),
      ]);
      await tester.pumpWidget(_app(h));
      await tester.pumpAndSettle();

      expect(
        find.text("Can't reach bob@example.com. Sign in again"),
        findsOneWidget,
      );
      expect(find.text(Copy.signInAgain), findsOneWidget);
      expect(find.text('Sender One'), findsOneWidget); // card still loads
    },
  );

  testWidgets('FD-03 AC2 phase_changed shows the up to date divider once', (
    tester,
  ) async {
    final h = await _make();
    h.api.feedPages.addAll([
      pageOf(
        [
          buildCard(messageId: 'a', senderName: 'First'),
          buildCard(messageId: 'b', senderName: 'Second'),
        ],
        nextCursor: 'c1',
        phaseChanged: true,
      ),
      pageOf(
        [buildCard(messageId: 'c', senderName: 'Third')],
        nextCursor: 'c1',
        phaseChanged: true,
      ),
    ]);
    await tester.pumpWidget(_app(h));
    await tester.pumpAndSettle();

    expect(find.text(Copy.upToDate), findsOneWidget);

    await tester.tap(find.text(Copy.continueLabel));
    await tester.pumpAndSettle();

    // Divider shown once: dismissing it does not bring it back, even though
    // the second page also reported phase_changed.
    expect(find.text(Copy.upToDate), findsNothing);
    expect(find.text('First'), findsOneWidget);
  });

  testWidgets(
    'FD-03 AC5 opening the Feed and pulling to refresh send refresh true',
    (tester) async {
      final h = await _make();
      await tester.pumpWidget(_app(h));
      await tester.pumpAndSettle();

      await h.model.refresh();
      await tester.pumpAndSettle();

      final nextCalls = h.api.calls
          .where((c) => c.path == '/api/v1/feed/next')
          .toList();
      expect(nextCalls, hasLength(2));
      for (final call in nextCalls) {
        expect((call.body as Map<String, Object?>)['refresh'], isTrue);
      }
    },
  );

  testWidgets('s9_feed_loading shows a progress indicator', (tester) async {
    final h = await _make();
    h.api.feedGate = Completer<void>();
    await tester.pumpWidget(_app(h));
    await tester.pump();

    expect(find.byType(CircularProgressIndicator), findsOneWidget);
    expect(
      find.byWidgetPredicate(
        (w) => w is Semantics && w.properties.label == Copy.loadingCards,
      ),
      findsOneWidget,
    );

    h.api.feedGate!.complete();
    await tester.pumpAndSettle();
  });

  testWidgets('s9_feed_normal shows the next card behind', (tester) async {
    final h = await _make();
    h.api.feedPages.addAll([
      pageOf([
        buildCard(messageId: 'a', senderName: 'First'),
        buildCard(messageId: 'b', senderName: 'Second'),
      ]),
    ]);
    await tester.pumpWidget(_app(h));
    await tester.pumpAndSettle();

    expect(find.text('First'), findsOneWidget); // in focus
    expect(find.text('Second'), findsOneWidget); // behind
  });

  testWidgets('s9_feed_empty shows Nothing to triage', (tester) async {
    final h = await _make();
    h.api.feedPages.addAll([emptyFeedPage()]);
    await tester.pumpWidget(_app(h));
    await tester.pumpAndSettle();

    expect(find.text(Copy.nothingToTriage), findsOneWidget);
  });

  testWidgets(
    's9_feed_all_mailboxes_need_sign_in shows the full-screen prompt',
    (tester) async {
      final h = await _make(mailboxes: const [_jane]);
      h.api.feedPages.addAll([
        emptyFeedPage(mailboxErrors: [signInError('mb-1')]),
      ]);
      await tester.pumpWidget(_app(h));
      await tester.pumpAndSettle();

      expect(find.text(Copy.allNeedSignIn), findsOneWidget);
      expect(find.text(Copy.signInAgainTo('jane@example.com')), findsOneWidget);
    },
  );

  testWidgets('s9_feed_offline shows the offline banner', (tester) async {
    final h = await _make(online: false);
    h.api.nextError = const NetworkException();
    await tester.pumpWidget(_app(h));
    // The loading spinner animates, so pump frames rather than settle.
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 100));

    expect(find.text(Copy.offline), findsOneWidget);
  });

  testWidgets('s9_feed_load_failed shows Try again', (tester) async {
    final h = await _make();
    h.api.nextError = ApiException(
      status: 502,
      code: 'provider_unavailable',
      requestId: 'r1',
    );
    await tester.pumpWidget(_app(h));
    await tester.pumpAndSettle();

    expect(find.text(Copy.actionFailed), findsOneWidget);
    expect(find.text(Copy.tryAgain), findsOneWidget);

    // Tap Try again: nextError was consumed, so a fresh load succeeds.
    h.api.feedPages.addAll([
      pageOf([buildCard(senderName: 'Recovered')]),
    ]);
    await tester.tap(find.text(Copy.tryAgain));
    await tester.pumpAndSettle();

    expect(find.text(Copy.actionFailed), findsNothing);
    expect(find.text('Recovered'), findsOneWidget);
  });

  testWidgets('s9_feed_bulk_badge_tap shows the reason', (tester) async {
    final h = await _make();
    final reason =
        'Mailing list, sent through Mailchimp, has one-click unsubscribe.';
    h.api.feedPages.addAll([
      pageOf([buildCard(bulkScore: 77, bulkReason: reason)]),
    ]);
    await tester.pumpWidget(_app(h));
    await tester.pumpAndSettle();

    await tester.tap(find.text(Copy.bulkBadge(77)));
    await tester.pumpAndSettle();

    expect(find.text(reason), findsOneWidget);
  });

  testWidgets('reconnect banner action starts reconnect for that mailbox', (
    tester,
  ) async {
    final h = await _make(mailboxes: const [_jane, _bob]);
    h.api.feedPages.addAll([
      pageOf(
        [buildCard(messageId: 'a', mailboxId: 'mb-1')],
        mailboxErrors: [signInError('mb-2')],
      ),
    ]);
    await tester.pumpWidget(_app(h));
    await tester.pumpAndSettle();

    await tester.tap(find.text(Copy.signInAgain));
    await tester.pumpAndSettle();

    final start = h.api.calls.where(
      (c) => c.path == '/api/v1/auth/google/start',
    );
    expect(start, hasLength(1));
    final body = start.single.body as Map<String, Object?>;
    expect(body['intent'], 'reconnect');
    expect(body['mailbox_id'], 'mb-2');
    expect(h.browser.assigned, hasLength(1));
  });

  testWidgets('asvs_v1_1_2 mail fields are rendered through Text widgets', (
    tester,
  ) async {
    final h = await _make();
    h.api.feedPages.addAll([
      pageOf([
        buildCard(
          senderName: 'Rich <b>Name</b>',
          senderAddress: 'rich@example.com',
          subject: 'Subject <i>x</i>',
          preview: 'Body with <a href="https://example.com">link</a>',
        ),
      ]),
    ]);
    await tester.pumpWidget(_app(h));
    await tester.pumpAndSettle();

    expect(find.text('Rich <b>Name</b>'), findsOneWidget);
    expect(find.text('Rich <b>Name</b>'), findsWidgets);
    expect(
      find
          .byType(Text)
          .evaluate()
          .any((e) => (e.widget as Text).data == 'Rich <b>Name</b>'),
      isTrue,
    );
    // No rich text or selectable "mail" widgets render the fields.
    expect(find.byType(SelectableText), findsNothing);
  });

  testWidgets(
    'asvs_v3_2_2 hostile subject and sender name render as literal text',
    (tester) async {
      final hostile = '<script>alert(1)</script>\u202Eevil text\u202C';
      final longSubject = '<b>${'x' * 996}</b>';
      final h = await _make();
      h.api.feedPages.addAll([
        pageOf([buildCard(senderName: hostile, subject: longSubject)]),
      ]);
      await tester.pumpWidget(_app(h));
      await tester.pumpAndSettle();

      expect(find.text(hostile), findsOneWidget);
      expect(find.text(longSubject), findsOneWidget);
      expect(find.byType(SelectableText), findsNothing);
    },
  );

  testWidgets('XC-03 feed controls are labelled', (tester) async {
    final h = await _make(mailboxes: const [_jane, _bob]);
    h.api.feedPages.addAll([
      pageOf(
        [buildCard(bulkScore: 44, bulkReason: 'reason text')],
        mailboxErrors: [signInError('mb-2')],
      ),
    ]);
    await tester.pumpWidget(_app(h));
    await tester.pumpAndSettle();

    expect(
      find.byWidgetPredicate(
        (w) =>
            w is Semantics && w.properties.label == Copy.bulkBadgeSemantics(44),
      ),
      findsWidgets,
    );
    expect(
      find.byWidgetPredicate(
        (w) => w is Semantics && w.properties.label == Copy.signInAgain,
      ),
      findsWidgets,
    );
  });
}
