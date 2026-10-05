import 'package:app/api/fake_api_client.dart';
import 'package:app/api/models/feed.dart';
import 'package:app/api/models/session.dart';
import 'package:app/api/models/swipe.dart';
import 'package:app/copy.dart';
import 'package:app/screens/feed/blitz_results_card.dart';
import 'package:app/screens/feed/feed_screen.dart';
import 'package:app/screens/feed/swipeable_card.dart';
import 'package:app/state/blitz_model.dart';
import 'package:app/state/feed_model.dart';
import 'package:app/state/id_generator.dart';
import 'package:app/state/session_model.dart';
import 'package:app/state/swipe_controller.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import '../support/cards.dart';
import '../support/fake_browser.dart';
import '../support/fake_connectivity.dart';

class _H {
  _H() {
    api.session = const Session(
      state: SessionState.authenticated,
      user: SessionUser(userId: 'u1', isAdmin: false),
      mailboxes: [
        Mailbox(
          mailboxId: 'mb-1',
          provider: 'gmail',
          emailAddress: 'jane@example.com',
          status: MailboxStatus.connected,
        ),
      ],
    );
    session = SessionModel(api: api);
    model = FeedModel(
      api: api,
      session: session,
      connectivity: FakeConnectivity(online: true),
    );
  }

  final api = FakeApiClient();
  final browser = FakeBrowser();
  late final SessionModel session;
  late final FeedModel model;
}

SwipeResult _result(
  String outcome, {
  List<BlockPrompt> prompts = const [],
  String undo = 'u',
}) => SwipeResult(
  outcome: outcome,
  undoToken: undo,
  prompts: prompts,
  achievementsUnlocked: const [],
  bossDefeated: false,
);

Future<_H> _open(WidgetTester tester, List<FeedCard> cards) async {
  final h = _H();
  await h.session.refresh();
  h.api.feedPages.add(pageOf(cards));
  await tester.pumpWidget(
    MaterialApp(
      home: Scaffold(
        body: FeedScreen(
          model: h.model,
          session: h.session,
          api: h.api,
          browser: h.browser,
        ),
      ),
    ),
  );
  await tester.pumpAndSettle();
  return h;
}

List<FeedCard> _cards(int n) => [
  for (var i = 0; i < n; i++) buildCard(messageId: 'm$i', subject: 'Subj $i'),
];

Future<void> _start(WidgetTester tester) async {
  await tester.tap(find.text(Copy.blitz));
  await tester.pump();
}

/// Taps a swipe button and lets the fly-off and the ack finish without
/// settling (the Blitz timer never lets the tree settle).
Future<void> _tapSwipe(WidgetTester tester, String label) async {
  await tester.tap(find.byTooltip(label));
  await tester.pump();
  await tester.pump(kFlyOffDuration);
  await tester.pump(const Duration(milliseconds: 50));
}

Future<void> _teardown(WidgetTester tester) async {
  await tester.pumpWidget(const SizedBox.shrink());
}

void main() {
  testWidgets('GM-07 AC1 Blitz starts a 60 second round with timer and score', (
    tester,
  ) async {
    await _open(tester, _cards(3));
    await _start(tester);
    expect(find.text('1:00'), findsOneWidget);
    expect(find.text(Copy.blitzScore(0)), findsOneWidget);
    await tester.pump(const Duration(seconds: 15));
    expect(find.text('0:45'), findsOneWidget);
    await _teardown(tester);
  });

  testWidgets('GM-07 AC1 round ends at zero with a results card', (
    tester,
  ) async {
    await _open(tester, _cards(3));
    await _start(tester);
    await tester.pump(kBlitzLength);
    expect(find.text(Copy.blitzOver), findsOneWidget);
    expect(find.byType(BlitzResultsCard), findsOneWidget);
    await _teardown(tester);
  });

  testWidgets('GM-07 AC2 personal cards are not shown during the round', (
    tester,
  ) async {
    await _open(tester, [
      buildCard(
        messageId: 'p',
        subject: 'Personal one',
        messageClass: MessageClass.personal,
      ),
      buildCard(messageId: 'b', subject: 'Bulk one'),
    ]);
    expect(find.text('Personal one'), findsOneWidget);
    await _start(tester);
    expect(find.text('Personal one'), findsNothing);
    expect(find.text('Bulk one'), findsOneWidget);
    await _teardown(tester);
  });

  testWidgets('GM-07 AC2 personal cards return after the round', (
    tester,
  ) async {
    final h = await _open(tester, [
      buildCard(
        messageId: 'p',
        subject: 'Personal one',
        messageClass: MessageClass.personal,
      ),
      buildCard(messageId: 'b', subject: 'Bulk one'),
    ]);
    await _start(tester);
    await tester.tap(find.text(Copy.endRound));
    await tester.pump();
    await tester.tap(find.text(Copy.blitzDone));
    await tester.pump();
    expect(find.text('Personal one'), findsOneWidget);
    // Never skipped on the server: only swipes are sent, and none were.
    expect(h.api.calls.where((c) => c.path == '/api/v1/swipes'), isEmpty);
    await _teardown(tester);
  });

  testWidgets('GM-07 AC3 reject toast states the delay and undo works in a '
      'round', (tester) async {
    final h = await _open(tester, [
      buildCard(
        messageId: 'l',
        subject: 'List mail',
        messageClass: MessageClass.list,
        hasOneClick: true,
      ),
      buildCard(messageId: 'b', subject: 'Bulk one'),
    ]);
    h.api.swipeResults.add(_result('trashed_unsubscribe_queued'));
    await _start(tester);
    await _tapSwipe(tester, Copy.rejectButton);
    expect(find.text(Copy.trashedUnsubscribing(kUnsubDelay)), findsOneWidget);
    expect(find.text(Copy.blitzScore(1)), findsOneWidget);
    await tester.tap(find.byTooltip(Copy.undoButton));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 300));
    expect(find.text('List mail'), findsOneWidget);
    expect(find.text(Copy.restored), findsOneWidget);
    expect(find.text(Copy.blitzScore(0)), findsOneWidget);
    await _teardown(tester);
  });

  testWidgets('GM-07 AC4 block prompts are held until the round ends then '
      'shown in turn', (tester) async {
    final h = await _open(tester, _cards(3));
    h.api.swipeResults.addAll([
      _result(
        'trashed',
        prompts: const [
          BlockPrompt(promptRef: 'p1', senderName: 'Spammer'),
          BlockPrompt(promptRef: 'p2', senderName: 'Pest'),
        ],
      ),
    ]);
    await _start(tester);
    await _tapSwipe(tester, Copy.rejectButton);
    expect(find.text(Copy.blockQuestion('Spammer')), findsNothing);
    await tester.tap(find.text(Copy.endRound));
    await tester.pump();
    expect(find.text(Copy.blockQuestion('Spammer')), findsNothing);
    await tester.tap(find.text(Copy.blitzDone));
    await tester.pumpAndSettle();
    expect(find.text(Copy.blockQuestion('Spammer')), findsOneWidget);
    await tester.tap(find.text(Copy.notNow));
    await tester.pumpAndSettle();
    expect(find.text(Copy.blockQuestion('Pest')), findsOneWidget);
    await tester.tap(find.text(Copy.notNow));
    await tester.pumpAndSettle();
    expect(h.api.lastDeclinedPromptRef, 'p2');
    await _teardown(tester);
  });

  testWidgets('GM-07 AC5 nothing about the round survives dismissing the '
      'results', (tester) async {
    await _open(tester, _cards(3));
    await _start(tester);
    await _tapSwipe(tester, Copy.keepButton);
    await tester.tap(find.text(Copy.endRound));
    await tester.pump();
    expect(find.text('Score'), findsOneWidget);
    await tester.tap(find.text(Copy.blitzDone));
    await tester.pump();
    expect(find.byType(BlitzResultsCard), findsNothing);
    expect(find.text(Copy.blitz), findsOneWidget);
    expect(find.textContaining('Score'), findsNothing);
    await _teardown(tester);
  });

  testWidgets('GM-07 AC5 the model clears its result and score', (
    tester,
  ) async {
    final h = await _open(tester, _cards(2));
    final swipes = SwipeController(
      api: h.api,
      feed: h.model,
      ids: IdGenerator(),
    );
    final blitz = BlitzModel(feed: h.model, swipes: swipes)..start();
    blitz.end();
    expect(blitz.result, isNotNull);
    await blitz.dismissResults();
    expect(blitz.result, isNull);
    expect(blitz.score, 0);
    expect(blitz.status, BlitzStatus.idle);
    blitz.dispose();
    swipes.dispose();
    await _teardown(tester);
  });

  testWidgets('s9_blitz_running', (tester) async {
    await _open(tester, _cards(2));
    await _start(tester);
    expect(find.text(Copy.endRound), findsOneWidget);
    expect(find.text(Copy.blitz), findsNothing);
    await _teardown(tester);
  });

  testWidgets('s9_blitz_ended', (tester) async {
    await _open(tester, _cards(2));
    await _start(tester);
    await _tapSwipe(tester, Copy.keepButton);
    await tester.tap(find.text(Copy.endRound));
    await tester.pump();
    expect(find.text(Copy.blitzOver), findsOneWidget);
    for (final label in [
      Copy.blitzScoreRow,
      Copy.blitzCleared,
      Copy.blitzKept,
      Copy.blitzFiled,
    ]) {
      expect(find.text(label), findsOneWidget);
    }
    expect(find.text('1'), findsNWidgets(2)); // score and kept
    expect(find.text('0'), findsNWidgets(2)); // cleared and filed
    await _teardown(tester);
  });

  testWidgets('s9_blitz_interrupted pauses when the tab is hidden', (
    tester,
  ) async {
    await _open(tester, _cards(2));
    await _start(tester);
    await tester.pump(const Duration(seconds: 10));
    expect(find.text('0:50'), findsOneWidget);
    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.hidden);
    await tester.pump(const Duration(seconds: 20));
    expect(find.text('0:50'), findsOneWidget);
    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.inactive);
    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.resumed);
    await tester.pump();
    await tester.pump(const Duration(seconds: 5));
    expect(find.text('0:45'), findsOneWidget);
    await _teardown(tester);
  });

  testWidgets('End round ends the round early', (tester) async {
    await _open(tester, _cards(2));
    await _start(tester);
    await tester.pump(const Duration(seconds: 5));
    await tester.tap(find.text(Copy.endRound));
    await tester.pump();
    expect(find.byType(BlitzResultsCard), findsOneWidget);
    await _teardown(tester);
  });

  testWidgets('GM-07 AC1 leaving the Feed mid-round leaves no timer or hold', (
    tester,
  ) async {
    await _open(tester, _cards(2));
    await _start(tester);
    await _teardown(tester);
    // A leaked periodic timer would fail the test at teardown.
    expect(tester.binding.transientCallbackCount, 0);
  });

  testWidgets('XC-03 Blitz controls are labelled', (tester) async {
    await _open(tester, _cards(2));
    expect(find.bySemanticsLabel(Copy.blitzSemantics), findsOneWidget);
    await _start(tester);
    expect(find.bySemanticsLabel('1:00'), findsOneWidget);
    expect(find.bySemanticsLabel(Copy.blitzScore(0)), findsOneWidget);
    expect(find.bySemanticsLabel(Copy.endRound), findsOneWidget);
    await _teardown(tester);
  });
}
