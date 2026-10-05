import 'package:app/api/api_client.dart';
import 'package:app/api/fake_api_client.dart';
import 'package:app/api/models/feed.dart';
import 'package:app/api/models/session.dart';
import 'package:app/api/models/swipe.dart';
import 'package:app/copy.dart';
import 'package:app/screens/feed/feed_screen.dart';
import 'package:app/screens/feed/swipeable_card.dart';
import 'package:app/state/feed_model.dart';
import 'package:app/state/session_model.dart';
import 'package:app/state/swipe_controller.dart';
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

class _Harness {
  _Harness({bool online = true}) {
    api = FakeApiClient();
    api.session = const Session(
      state: SessionState.authenticated,
      user: SessionUser(userId: 'u1', isAdmin: false),
      mailboxes: [_jane],
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

Widget _app(_Harness h, {bool disableAnimations = false}) {
  return MaterialApp(
    home: Scaffold(
      body: Builder(
        builder: (context) {
          final screen = FeedScreen(
            model: h.model,
            session: h.session,
            api: h.api,
            browser: h.browser,
          );
          if (!disableAnimations) return screen;
          return MediaQuery(
            data: MediaQuery.of(context).copyWith(disableAnimations: true),
            child: screen,
          );
        },
      ),
    ),
  );
}

Future<_Harness> _make({bool online = true}) async {
  final h = _Harness(online: online);
  await h.session.refresh();
  return h;
}

SwipeResult _result(String outcome, {String undoToken = 'undo-1'}) {
  return SwipeResult(
    outcome: outcome,
    undoToken: undoToken,
    prompts: const [],
    achievementsUnlocked: const [],
    bossDefeated: false,
  );
}

/// Loads a single card and pumps the feed to a ready state.
Future<void> _pumpCard(
  WidgetTester tester,
  _Harness h, {
  FeedCard? card,
  bool disableAnimations = false,
}) async {
  h.api.feedPages.addAll([
    pageOf([card ?? buildCard()], nextCursor: 'c1'),
  ]);
  await tester.pumpWidget(_app(h, disableAnimations: disableAnimations));
  await tester.pumpAndSettle();
}

/// Loads two cards (one page each) so the model can refill after the first
/// swipe, then pumps to a ready state.
Future<void> _pumpTwoCards(WidgetTester tester, _Harness h) async {
  h.api.feedPages.addAll([
    pageOf([buildCard(messageId: 'm1')], nextCursor: 'c1'),
    pageOf([buildCard(messageId: 'm2')], nextCursor: 'c1'),
  ]);
  await tester.pumpWidget(_app(h));
  await tester.pumpAndSettle();
}

/// Drags the focused card by [offset] as a real finger would: many small move
/// events with time between them, so the pan recognizer gets updates and a
/// real release velocity.
Future<void> _dragCard(WidgetTester tester, Offset offset) async {
  final gesture = await tester.startGesture(
    tester.getCenter(find.byType(SwipeableCard)),
  );
  const steps = 10;
  for (var i = 0; i < steps; i++) {
    await gesture.moveBy(offset / steps.toDouble());
    await tester.pump(const Duration(milliseconds: 16));
  }
  await gesture.up();
}

/// Drags the focused card by [offset] and settles the fly-off.
Future<void> _swipe(WidgetTester tester, Offset offset) async {
  await _dragCard(tester, offset);
  await tester.pumpAndSettle();
}

List<FakeCall> _swipeCalls(_Harness h) =>
    h.api.calls.where((c) => c.path == '/api/v1/swipes').toList();

String _action(_Harness h, int index) =>
    (_swipeCalls(h)[index].body as Map<String, Object?>)['action'] as String;

void main() {
  testWidgets('SW-01 AC1 swipe right and Keep both send keep', (tester) async {
    final h = await _make();
    await _pumpTwoCards(tester, h);

    // Keep button on the first card.
    await tester.tap(find.byTooltip(Copy.keepButton));
    await tester.pumpAndSettle();
    expect(_action(h, 0), 'keep');

    // Right swipe on the next card.
    await _swipe(tester, const Offset(300, 0));
    expect(_action(h, 1), 'keep');
  });

  testWidgets('SW-02 AC1 swipe down and Skip both send skip', (tester) async {
    final h = await _make();
    await _pumpTwoCards(tester, h);

    await tester.tap(find.byTooltip(Copy.skipButton));
    await tester.pumpAndSettle();
    expect(_action(h, 0), 'skip');

    await _swipe(tester, const Offset(0, 300));
    expect(_action(h, 1), 'skip');
  });

  testWidgets('SW-03 AC1 swipe left and Reject both send reject', (
    tester,
  ) async {
    final h = await _make();
    await _pumpTwoCards(tester, h);

    await tester.tap(find.byTooltip(Copy.rejectButton));
    await tester.pumpAndSettle();
    expect(_action(h, 0), 'reject');

    await _swipe(tester, const Offset(-300, 0));
    expect(_action(h, 1), 'reject');
  });

  testWidgets('SW-04 AC1 swipe up and File both open the filing launcher', (
    tester,
  ) async {
    final h = await _make();
    await _pumpCard(tester, h);

    // File button: the default launcher returns null, so nothing is sent.
    await tester.tap(find.byTooltip(Copy.fileButton));
    await tester.pumpAndSettle();
    expect(_swipeCalls(h), isEmpty);

    // Up swipe: also opens the launcher (returns null), nothing sent.
    await _swipe(tester, const Offset(0, -300));
    expect(_swipeCalls(h), isEmpty);
  });

  testWidgets('SW-03 AC2 reject toast states the delay', (tester) async {
    final h = await _make();
    h.api.swipeResults.addAll([
      _result('trashed_unsubscribe_queued', undoToken: 'u1'),
    ]);
    await _pumpCard(
      tester,
      h,
      card: buildCard(messageClass: MessageClass.list, hasOneClick: true),
    );

    await tester.tap(find.byTooltip(Copy.rejectButton));
    await tester.pumpAndSettle();

    expect(find.text(Copy.trashedUnsubscribing(kUnsubDelay)), findsOneWidget);
  });

  testWidgets(
    'SW-03 AC2 toast follows the server outcome for a mailto unsubscribe',
    (tester) async {
      final h = await _make();
      final dueAt = DateTime.now().toUtc().add(const Duration(minutes: 5));
      h.api.swipeResults.addAll([
        SwipeResult(
          outcome: 'trashed_unsubscribe_queued',
          unsubscribeDueAt: dueAt,
          undoToken: 'u1',
          prompts: const [],
          achievementsUnlocked: const [],
          bossDefeated: false,
        ),
      ]);
      await _pumpCard(
        tester,
        h,
        card: buildCard(
          messageClass: MessageClass.list,
          hasOneClick: false, // mailto list: no one-click, still queues
        ),
      );

      await tester.tap(find.byTooltip(Copy.rejectButton));
      await tester.pumpAndSettle();

      expect(
        find.text(Copy.trashedUnsubscribing(const Duration(minutes: 5))),
        findsOneWidget,
      );
    },
  );

  testWidgets('SW-03 AC3 reported_spam toast', (tester) async {
    final h = await _make();
    h.api.swipeResults.addAll([_result('reported_spam')]);
    await _pumpCard(tester, h);

    await tester.tap(find.byTooltip(Copy.rejectButton));
    await tester.pumpAndSettle();

    expect(find.text(Copy.reportedSpam), findsOneWidget);
  });

  testWidgets('SW-05 AC1 undo restores the card to the top', (tester) async {
    final h = await _make();
    h.api.swipeResults.addAll([_result('kept', undoToken: 'u1')]);
    await _pumpCard(tester, h, card: buildCard(senderName: 'First'));

    await tester.tap(find.byTooltip(Copy.keepButton));
    await tester.pumpAndSettle();
    expect(find.text('First'), findsNothing);

    await tester.tap(find.byTooltip(Copy.undoButton));
    await tester.pumpAndSettle();

    expect(find.text('First'), findsOneWidget);
    expect(find.text(Copy.restored), findsOneWidget);
  });

  testWidgets(
    'SW-05 AC3 undo after the unsubscribe went says it had already been sent',
    (tester) async {
      final h = await _make();
      h.api.swipeResults.addAll([_result('trashed', undoToken: 'u1')]);
      h.api.undoResults.addAll([
        const UndoResult(restored: true, unsubscribeAlreadySent: true),
      ]);
      await _pumpCard(tester, h);

      await tester.tap(find.byTooltip(Copy.rejectButton));
      await tester.pumpAndSettle();
      await tester.tap(find.byTooltip(Copy.undoButton));
      await tester.pumpAndSettle();

      expect(find.text(Copy.restoredAlreadySent), findsOneWidget);
    },
  );

  testWidgets(
    'SW-05 AC4a undo of a spam report says the report cannot be recalled',
    (tester) async {
      final h = await _make();
      h.api.swipeResults.addAll([_result('reported_spam', undoToken: 'u1')]);
      await _pumpCard(tester, h);

      await tester.tap(find.byTooltip(Copy.rejectButton));
      await tester.pumpAndSettle();
      await tester.tap(find.byTooltip(Copy.undoButton));
      await tester.pumpAndSettle();

      expect(find.text(Copy.restoredSpam), findsOneWidget);
    },
  );

  testWidgets('FD-04 AC1 message_changed drops the card silently', (
    tester,
  ) async {
    final h = await _make();
    h.api.nextSwipeError = ApiException(
      status: 409,
      code: 'message_changed',
      requestId: 'r1',
    );
    await _pumpCard(tester, h, card: buildCard(senderName: 'Gone'));

    await tester.tap(find.byTooltip(Copy.rejectButton));
    await tester.pumpAndSettle();

    // Card stays gone, no error toast.
    expect(find.text('Gone'), findsNothing);
    expect(find.text(Copy.actionFailed), findsNothing);
  });

  testWidgets('PB-01 AC1 block prompt shows with Block preselected', (
    tester,
  ) async {
    final h = await _make();
    h.api.swipeResults.addAll([
      const SwipeResult(
        outcome: 'trashed',
        undoToken: 'u1',
        prompts: [BlockPrompt(promptRef: 'p1', senderName: 'Spammer')],
        achievementsUnlocked: [],
        bossDefeated: false,
      ),
    ]);
    await _pumpCard(tester, h, card: buildCard(senderName: 'Spammer'));

    await tester.tap(find.byTooltip(Copy.rejectButton));
    await tester.pumpAndSettle();

    expect(find.text(Copy.blockQuestion('Spammer')), findsOneWidget);
    final blockButton = tester.widget<FilledButton>(
      find.widgetWithText(FilledButton, Copy.block),
    );
    expect(blockButton.autofocus, isTrue);
  });

  testWidgets('PB-01 AC3 Not now sends decline', (tester) async {
    final h = await _make();
    h.api.swipeResults.addAll([
      const SwipeResult(
        outcome: 'trashed',
        undoToken: 'u1',
        prompts: [BlockPrompt(promptRef: 'p1', senderName: 'Spammer')],
        achievementsUnlocked: [],
        bossDefeated: false,
      ),
    ]);
    await _pumpCard(tester, h);

    await tester.tap(find.byTooltip(Copy.rejectButton));
    await tester.pumpAndSettle();
    await tester.tap(find.text(Copy.notNow));
    await tester.pumpAndSettle();

    expect(h.api.lastDeclinedPromptRef, 'p1');
    expect(h.api.lastBlockedPromptRef, isNull);
  });

  testWidgets('XC-03 every swipe has a labelled button', (tester) async {
    final h = await _make();
    await _pumpCard(tester, h);

    for (final label in [
      Copy.rejectButton,
      Copy.skipButton,
      Copy.fileButton,
      Copy.keepButton,
      Copy.undoButton,
    ]) {
      expect(
        find.byWidgetPredicate(
          (w) => w is Semantics && w.properties.label == label,
        ),
        findsOneWidget,
      );
    }
  });

  testWidgets('XC-03 reduced motion skips the fly-off animation', (
    tester,
  ) async {
    final h = await _make();
    // Reduced motion from the first pump: MediaQueryData(disableAnimations).
    await _pumpCard(tester, h, disableAnimations: true);

    await _dragCard(tester, const Offset(300, 0));
    await tester.pump();

    // No fly-off: the swipe commits at once, so the request is already sent
    // and the card has left the feed after a single pump.
    expect(_action(h, 0), 'keep');
    expect(find.byType(SwipeableCard), findsNothing);
  });

  testWidgets('XC-04 provider error returns the card with Couldn\'t do that', (
    tester,
  ) async {
    final h = await _make();
    h.api.nextSwipeError = ApiException(
      status: 502,
      code: 'provider_unavailable',
      requestId: 'r1',
    );
    await _pumpCard(tester, h, card: buildCard(senderName: 'Back'));

    await tester.tap(find.byTooltip(Copy.rejectButton));
    await tester.pumpAndSettle();

    expect(find.text(Copy.actionFailed), findsOneWidget);
    expect(find.text('Back'), findsOneWidget); // card returned to the top
  });

  testWidgets('s9_feed_action_failed card returns to the top', (tester) async {
    final h = await _make();
    h.api.nextSwipeError = ApiException(
      status: 500,
      code: 'internal',
      requestId: 'r1',
    );
    await _pumpCard(tester, h, card: buildCard(senderName: 'Back'));

    await tester.tap(find.byTooltip(Copy.keepButton));
    await tester.pumpAndSettle();

    expect(find.text(Copy.actionFailed), findsOneWidget);
    expect(find.text('Back'), findsOneWidget);
  });

  testWidgets('s9_feed_message_changed next card shows', (tester) async {
    final h = await _make();
    h.api.nextSwipeError = ApiException(
      status: 409,
      code: 'message_changed',
      requestId: 'r1',
    );
    h.api.feedPages.addAll([
      pageOf([
        buildCard(messageId: 'a', senderName: 'Gone'),
        buildCard(messageId: 'b', senderName: 'Next'),
      ]),
    ]);
    await tester.pumpWidget(_app(h));
    await tester.pumpAndSettle();

    await tester.tap(find.byTooltip(Copy.rejectButton));
    await tester.pumpAndSettle();

    expect(find.text('Gone'), findsNothing);
    expect(find.text('Next'), findsOneWidget);
  });

  testWidgets('s9_feed_undo_disabled before any swipe', (tester) async {
    final h = await _make();
    await _pumpCard(tester, h);

    final undo = tester.widget<IconButton>(
      find.widgetWithIcon(IconButton, Icons.undo),
    );
    expect(undo.onPressed, isNull);
  });

  testWidgets('s9_feed_toast_manual_unsubscribe names Needs Attention', (
    tester,
  ) async {
    final h = await _make();
    h.api.swipeResults.addAll([_result('trashed_unsubscribe_manual')]);
    await _pumpCard(tester, h);

    await tester.tap(find.byTooltip(Copy.rejectButton));
    await tester.pumpAndSettle();

    expect(find.text(Copy.trashedUnsubscribeManual), findsOneWidget);
  });

  testWidgets('s9_feed_offline disables swipe buttons', (tester) async {
    final h = await _make(online: false);
    h.api.nextError = const NetworkException();
    await tester.pumpWidget(_app(h));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 100));

    expect(find.text(Copy.offline), findsOneWidget);
    for (final label in [
      Copy.rejectButton,
      Copy.skipButton,
      Copy.fileButton,
      Copy.keepButton,
    ]) {
      final button = tester.widget<IconButton>(
        find.ancestor(
          of: find.byIcon(_iconFor(label)),
          matching: find.byType(IconButton),
        ),
      );
      expect(button.onPressed, isNull, reason: '$label should be disabled');
    }
  });

  testWidgets('divider is dismissed by any swipe without an API call', (
    tester,
  ) async {
    final h = await _make();
    h.api.feedPages.addAll([
      pageOf([
        buildCard(messageId: 'a', senderName: 'First'),
      ], phaseChanged: true),
    ]);
    await tester.pumpWidget(_app(h));
    await tester.pumpAndSettle();

    // Divider is showing.
    expect(find.text(Copy.upToDate), findsOneWidget);

    // Any swipe dismisses it without an API call.
    await tester.tap(find.byTooltip(Copy.keepButton));
    await tester.pumpAndSettle();

    expect(find.text(Copy.upToDate), findsNothing);
    expect(_swipeCalls(h), isEmpty);
  });
}

IconData _iconFor(String label) {
  switch (label) {
    case Copy.rejectButton:
      return Icons.close;
    case Copy.skipButton:
      return Icons.arrow_downward;
    case Copy.fileButton:
      return Icons.folder_open;
    case Copy.keepButton:
      return Icons.check;
    default:
      return Icons.undo;
  }
}
