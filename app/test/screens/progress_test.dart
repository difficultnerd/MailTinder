import 'package:app/api/fake_api_client.dart';
import 'package:app/api/models/feed.dart';
import 'package:app/api/models/progress.dart';
import 'package:app/api/models/session.dart';
import 'package:app/api/models/swipe.dart';
import 'package:app/copy.dart';
import 'package:app/screens/feed/feed_screen.dart';
import 'package:app/state/feed_model.dart';
import 'package:app/state/progress_model.dart';
import 'package:app/state/round_tracker.dart';
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

class _H {
  _H() {
    api.session = const Session(
      state: SessionState.authenticated,
      user: SessionUser(userId: 'u1', isAdmin: false),
      mailboxes: [_jane],
    );
    session = SessionModel(api: api);
    model = FeedModel(
      api: api,
      session: session,
      connectivity: FakeConnectivity(online: true),
    );
    progress = ProgressModel(api: api);
  }

  final api = FakeApiClient();
  final browser = FakeBrowser();
  late final SessionModel session;
  late final FeedModel model;
  late final ProgressModel progress;
  final rounds = RoundTracker();
  final visible = ValueNotifier<bool>(true);

  Widget app() => MaterialApp(
    home: Scaffold(
      body: FeedScreen(
        key: UniqueKey(),
        model: model,
        session: session,
        api: api,
        browser: browser,
        progress: progress,
        rounds: rounds,
        feedVisible: visible,
      ),
    ),
  );

  int get progressCalls =>
      api.calls.where((c) => c.path == '/api/v1/progress').length;
}

Future<_H> _make() async {
  final h = _H();
  await h.session.refresh();
  return h;
}

List<FeedCard> _cards(int n, {BossInfo? boss, String name = 'Sender'}) => [
  for (var i = 0; i < n; i++)
    buildCard(messageId: 'm$i', senderName: name, boss: boss),
];

Future<void> _open(WidgetTester tester, _H h, List<FeedCard> cards) async {
  h.api.feedPages.add(pageOf(cards));
  await tester.pumpWidget(h.app());
  await tester.pumpAndSettle();
}

Future<void> _keep(WidgetTester tester, [int times = 1]) async {
  for (var i = 0; i < times; i++) {
    await tester.tap(find.byTooltip(Copy.keepButton));
    await tester.pumpAndSettle();
  }
}

SwipeResult _result({
  List<Achievement> achievements = const [],
  bool bossDefeated = false,
}) => SwipeResult(
  outcome: 'kept',
  undoToken: 'u',
  prompts: const [],
  achievementsUnlocked: achievements,
  bossDefeated: bossDefeated,
);

void main() {
  testWidgets('GM-01 AC1 meter shows total and change today', (tester) async {
    final h = await _make();
    h.api.progressQueue.addAll([
      const Progress(inboxCount: 12645, mailboxErrors: [], level: null),
      const Progress(inboxCount: 12431, mailboxErrors: [], level: null),
    ]);
    await _open(tester, h, _cards(1));
    expect(find.text('12,645, no change today'), findsOneWidget);
    await h.progress.load();
    await tester.pump();
    expect(find.text('12,431, down 214 today'), findsOneWidget);
    expect(Copy.meter(10, 5), '10, up 5 today');
  });

  testWidgets('GM-01 AC2 progress loads on open and after every 10 swipes', (
    tester,
  ) async {
    final h = await _make();
    await _open(tester, h, _cards(12));
    expect(h.progressCalls, 1);
    await _keep(tester, 9);
    expect(h.progressCalls, 1);
    await _keep(tester);
    expect(h.progressCalls, 2);
  });

  testWidgets('GM-01 AC2 failing mailbox shows the total with a marker', (
    tester,
  ) async {
    final h = await _make();
    h.api.progress = Progress(
      inboxCount: 100,
      mailboxErrors: [signInError('mb-1')],
      level: null,
    );
    await _open(tester, h, _cards(1));
    expect(find.text('100, no change today*'), findsOneWidget);
    expect(
      find.bySemanticsLabel('100, no change today. ${Copy.meterPartial}'),
      findsOneWidget,
    );
  });

  testWidgets('GM-03 AC1 round card after 50 swipes shows the five totals', (
    tester,
  ) async {
    final h = await _make();
    await _open(tester, h, _cards(50));
    await _keep(tester, 49);
    expect(find.text(Copy.roundOver), findsNothing);
    await _keep(tester);
    expect(find.text(Copy.roundOver), findsOneWidget);
    for (final label in [
      Copy.roundCleared,
      Copy.roundKept,
      Copy.roundFiled,
      Copy.roundUnsubscribed,
      Copy.roundBlocked,
    ]) {
      expect(find.text(label), findsOneWidget);
    }
    expect(find.text('50'), findsOneWidget);
    await tester.tap(find.text(Copy.keepGoing));
    await tester.pumpAndSettle();
    expect(find.text(Copy.roundOver), findsNothing);
    expect(h.rounds.swipesSinceCard, 0);
  });

  testWidgets('GM-03 AC1 round card at the up to date divider', (tester) async {
    final h = await _make();
    h.api.feedPages.addAll([
      pageOf([buildCard(messageId: 'a')], nextCursor: 'c1'),
      pageOf([buildCard(messageId: 'b')], phaseChanged: true),
    ]);
    await tester.pumpWidget(h.app());
    await tester.pumpAndSettle();
    await _keep(tester);
    expect(find.text(Copy.upToDate), findsOneWidget);
    expect(find.text(Copy.roundOver), findsOneWidget);
  });

  testWidgets('GM-03 AC1 round card after leaving the Feed', (tester) async {
    final h = await _make();
    await _open(tester, h, _cards(3));
    await _keep(tester);
    expect(find.text(Copy.roundOver), findsNothing);
    h.visible.value = false;
    await tester.pump();
    h.visible.value = true;
    await tester.pump();
    expect(find.text(Copy.roundOver), findsOneWidget);
  });

  testWidgets('GM-03 AC2 round totals start at zero in a new app instance', (
    tester,
  ) async {
    final first = await _make();
    await _open(tester, first, _cards(2));
    await _keep(tester);
    expect(first.rounds.totals.kept, 1);

    final second = await _make();
    await _open(tester, second, _cards(2));
    second.visible.value = false;
    await tester.pump();
    second.visible.value = true;
    await tester.pump();
    final t = second.rounds.totals;
    expect([
      t.cleared,
      t.kept,
      t.filed,
      t.unsubscribed,
      t.blocked,
    ], everyElement(0));
    expect(find.text(Copy.roundOver), findsNothing);
  });

  testWidgets('GM-04 AC1 level banner shows year and mail left', (
    tester,
  ) async {
    final h = await _make();
    h.api.progress = const Progress(
      inboxCount: 5,
      mailboxErrors: [],
      level: Level(year: 2023, remaining: 1840),
    );
    await _open(tester, h, _cards(1));
    expect(find.text('Level 2023: 1,840 left'), findsOneWidget);
  });

  testWidgets('GM-04 AC1 no banner while level is null', (tester) async {
    final h = await _make();
    await _open(tester, h, _cards(1));
    expect(find.textContaining('Level'), findsNothing);
  });

  testWidgets('GM-04 AC2 level complete shows when the year changes', (
    tester,
  ) async {
    final h = await _make();
    h.api.progressQueue.addAll([
      const Progress(
        inboxCount: 5,
        mailboxErrors: [],
        level: Level(year: 2024, remaining: 1),
      ),
      const Progress(
        inboxCount: 4,
        mailboxErrors: [],
        level: Level(year: 2023, remaining: 900),
      ),
    ]);
    await _open(tester, h, _cards(1));
    await h.progress.load();
    await tester.pump();
    expect(find.text(Copy.levelComplete(2024, 2023)), findsOneWidget);
    await tester.pump(const Duration(seconds: 3));
    expect(find.text(Copy.levelComplete(2024, 2023)), findsNothing);
  });

  testWidgets('GM-06 AC3 achievement celebration on the Feed', (tester) async {
    final h = await _make();
    h.api.swipeResults.add(
      _result(
        achievements: [
          Achievement(
            achievementId: 'first_unsubscribe',
            unlockedAt: DateTime.utc(2026, 1, 1),
          ),
          Achievement(
            achievementId: 'no_such_id',
            unlockedAt: DateTime.utc(2026, 1, 1),
          ),
        ],
      ),
    );
    await _open(tester, h, _cards(2));
    await tester.tap(find.byTooltip(Copy.keepButton));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 100));
    expect(
      find.text(Copy.achievementUnlocked('First unsubscribe')),
      findsOneWidget,
    );
    // Queued: the unknown ID shows after the first, with the plain prefix.
    await tester.tap(find.text(Copy.achievementUnlocked('First unsubscribe')));
    await tester.pump();
    expect(find.text('Achievement unlocked:'), findsOneWidget);
    await tester.pump(const Duration(seconds: 3));
    expect(find.textContaining('Achievement unlocked'), findsNothing);
  });

  testWidgets('GM-08 AC2 boss banner shows a health bar', (tester) async {
    final h = await _make();
    await _open(tester, h, [
      buildCard(
        messageId: 'a',
        senderName: 'Big Co',
        boss: const BossInfo(remaining: 10),
      ),
      buildCard(
        messageId: 'b',
        senderName: 'Big Co',
        boss: const BossInfo(remaining: 5),
      ),
    ]);
    expect(find.bySemanticsLabel('Boss Big Co: 10 left'), findsOneWidget);
    LinearProgressIndicator bar() => tester.widget<LinearProgressIndicator>(
      find.byType(LinearProgressIndicator),
    );
    expect(bar().value, 1.0);
    await _keep(tester);
    expect(find.bySemanticsLabel('Boss Big Co: 5 left'), findsOneWidget);
    expect(bar().value, 0.5);
  });

  testWidgets('GM-08 AC3 boss defeated celebration', (tester) async {
    final h = await _make();
    h.api.swipeResults.add(_result(bossDefeated: true));
    await _open(tester, h, _cards(2, name: 'Big Co'));
    await tester.tap(find.byTooltip(Copy.keepButton));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 100));
    expect(find.text(Copy.bossDefeated('Big Co')), findsOneWidget);
    await tester.pump(const Duration(seconds: 3));
  });

  testWidgets('XC-03 progress elements have semantics labels', (tester) async {
    final h = await _make();
    h.api.progress = const Progress(
      inboxCount: 7,
      mailboxErrors: [],
      level: Level(year: 2022, remaining: 3),
    );
    await _open(tester, h, [
      buildCard(boss: const BossInfo(remaining: 4), senderName: 'Big Co'),
    ]);
    expect(find.bySemanticsLabel('7, no change today'), findsOneWidget);
    expect(find.bySemanticsLabel('Level 2022: 3 left'), findsOneWidget);
    expect(find.bySemanticsLabel('Boss Big Co: 4 left'), findsOneWidget);
  });
}
