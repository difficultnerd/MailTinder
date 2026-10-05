import 'package:app/api/fake_api_client.dart';
import 'package:app/api/models/feed.dart';
import 'package:app/api/models/session.dart';
import 'package:app/api/models/swipe.dart';
import 'package:app/copy.dart';
import 'package:app/platform/sound_player.dart';
import 'package:app/screens/feed/effects.dart';
import 'package:app/screens/feed/feed_screen.dart';
import 'package:app/screens/feed/swipeable_card.dart';
import 'package:app/state/feed_model.dart';
import 'package:app/state/feedback_model.dart';
import 'package:app/state/play_prefs.dart';
import 'package:app/state/session_model.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import '../support/cards.dart';
import '../support/fake_browser.dart';
import '../support/fake_connectivity.dart';
import '../support/fake_haptics.dart';
import '../support/fake_sound_player.dart';

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
    feed = FeedModel(
      api: api,
      session: session,
      connectivity: FakeConnectivity(online: true),
    );
    feedback = FeedbackModel(prefs: prefs, sound: sound, haptics: haptics);
  }

  final FakeApiClient api = FakeApiClient();
  final FakeBrowser browser = FakeBrowser();
  final PlayPrefs prefs = PlayPrefs();
  final FakeSoundPlayer sound = FakeSoundPlayer();
  final FakeHaptics haptics = FakeHaptics();
  late final SessionModel session;
  late final FeedModel feed;
  late final FeedbackModel feedback;
}

Future<_H> _pump(
  WidgetTester tester, {
  bool reduced = false,
  FeedCard? card,
  int preCleared = 0,
}) async {
  final h = _H();
  await h.session.refresh();
  for (var i = 0; i < preCleared; i++) {
    h.feedback.onSwipe(buildCard(), SwipeKind.reject);
  }
  h.api.feedPages.addAll([
    pageOf([card ?? buildCard(messageId: 'm1')], nextCursor: 'c1'),
    pageOf([buildCard(messageId: 'm2')], nextCursor: 'c1'),
  ]);
  await tester.pumpWidget(
    MaterialApp(
      home: Scaffold(
        body: Builder(
          builder: (context) => MediaQuery(
            data: MediaQuery.of(context).copyWith(disableAnimations: reduced),
            child: FeedScreen(
              model: h.feed,
              session: h.session,
              api: h.api,
              browser: h.browser,
              feedback: h.feedback,
            ),
          ),
        ),
      ),
    ),
  );
  await tester.pumpAndSettle();
  return h;
}

/// Drags the card and releases, leaving the fly-off mid-flight.
Future<void> _release(WidgetTester tester, Offset offset) async {
  final gesture = await tester.startGesture(
    tester.getCenter(find.byType(SwipeableCard)),
  );
  for (var i = 0; i < 10; i++) {
    await gesture.moveBy(offset / 10);
    await tester.pump(const Duration(milliseconds: 16));
  }
  await gesture.up();
  await tester.pump(const Duration(milliseconds: 50));
}

Finder _effect(CardEffect e) => find.byKey(ValueKey('effect-${e.name}'));

void main() {
  testWidgets('GM-02 AC1 each direction plays its own animation', (
    tester,
  ) async {
    final cases = {
      const Offset(300, 0): CardEffect.flyRight,
      const Offset(-300, 0): CardEffect.flyLeft,
      const Offset(0, -300): CardEffect.flyUp,
      const Offset(0, 300): CardEffect.flyDown,
    };
    for (final entry in cases.entries) {
      await _pump(tester);
      await _release(tester, entry.key);
      expect(_effect(entry.value), findsOneWidget);
      await tester.pumpAndSettle(const Duration(seconds: 5));
      await tester.pumpWidget(const SizedBox.shrink());
    }
  });

  testWidgets('GM-02 AC1 reject on a high bulk score card plays the flush', (
    tester,
  ) async {
    await _pump(tester, card: buildCard(bulkScore: kFlushScore));
    await _release(tester, const Offset(-300, 0));
    expect(_effect(CardEffect.flush), findsOneWidget);
    expect(_effect(CardEffect.flyLeft), findsNothing);
    await tester.pumpAndSettle();
  });

  testWidgets('GM-02 AC2 no sound plays while Sounds is off', (tester) async {
    final h = await _pump(tester);
    await _release(tester, const Offset(300, 0));
    await tester.pumpAndSettle();
    expect(h.sound.played, isEmpty);
    expect(h.haptics.taps, 1);
  });

  testWidgets('GM-02 AC2 a sound plays per swipe when Sounds is on', (
    tester,
  ) async {
    final h = await _pump(tester);
    h.prefs.soundsOn = true;
    await _release(tester, const Offset(300, 0));
    await tester.pumpAndSettle();
    expect(h.sound.played, [SwipeSound.keep]);
  });

  testWidgets('GM-02 AC3 confetti at 100 cleared', (tester) async {
    final h = await _pump(tester, preCleared: kConfettiEvery - 1);
    expect(h.feedback.confettiDue, isFalse);
    await _release(tester, const Offset(-300, 0));
    expect(h.feedback.confettiDue, isTrue);
    expect(find.byKey(const ValueKey('confetti')), findsOneWidget);
    await tester.pump(kConfettiDuration + const Duration(milliseconds: 100));
    expect(h.feedback.confettiDue, isFalse);
    expect(find.byKey(const ValueKey('confetti')), findsNothing);
    await tester.pumpAndSettle();
  });

  testWidgets('GM-02 AC4 reduced motion runs no animation', (tester) async {
    final h = await _pump(tester, reduced: true);
    await _release(tester, const Offset(300, 0));
    expect(_effect(CardEffect.flyRight), findsNothing);
    expect(h.api.calls.where((c) => c.path == '/api/v1/swipes'), hasLength(1));
    await tester.pumpAndSettle();
    expect(tester.hasRunningAnimations, isFalse);
  });

  testWidgets('XC-03 reduced motion shows the static milestone instead of '
      'confetti', (tester) async {
    final h = await _pump(tester, reduced: true, preCleared: 99);
    await _release(tester, const Offset(-300, 0));
    expect(find.byKey(const ValueKey('confetti')), findsNothing);
    expect(find.text(Copy.clearedMilestone(100)), findsOneWidget);
    await tester.pump(kMilestoneTextDuration + const Duration(seconds: 1));
    expect(find.text(Copy.clearedMilestone(100)), findsNothing);
    expect(h.feedback.confettiDue, isFalse);
    await tester.pumpAndSettle();
  });

  testWidgets('GM-02 AC3 combo badge shows after 5 quick swipes', (
    tester,
  ) async {
    final h = await _pump(tester);
    for (var i = 0; i < kComboSwipes; i++) {
      h.feedback.onSwipe(buildCard(), SwipeKind.keep);
    }
    await tester.pump();
    expect(find.text(Copy.combo(kComboSwipes)), findsOneWidget);
  });

  testWidgets('GM-02 AC1 a button reject animates and counts toward 100 '
      'cleared', (tester) async {
    final h = await _pump(tester, preCleared: kConfettiEvery - 1);
    await tester.tap(find.byTooltip(Copy.rejectButton));
    await tester.pump(const Duration(milliseconds: 50));
    expect(_effect(CardEffect.flyLeft), findsOneWidget);
    expect(h.feedback.confettiDue, isTrue);
    await tester.pumpAndSettle();
    expect(h.api.calls.where((c) => c.path == '/api/v1/swipes'), hasLength(1));
  });

  testWidgets('GM-02 AC2 a button keep plays its sound when Sounds is on', (
    tester,
  ) async {
    final h = _H();
    h.prefs.soundsOn = true;
    await h.session.refresh();
    h.api.feedPages.addAll([
      pageOf([buildCard(messageId: 'm1')], nextCursor: 'c1'),
      pageOf([buildCard(messageId: 'm2')], nextCursor: 'c1'),
    ]);
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: FeedScreen(
            model: h.feed,
            session: h.session,
            api: h.api,
            browser: h.browser,
            feedback: h.feedback,
          ),
        ),
      ),
    );
    await tester.pumpAndSettle();
    await tester.tap(find.byTooltip(Copy.keepButton));
    await tester.pumpAndSettle();
    expect(h.sound.played, [SwipeSound.keep]);
  });
}
