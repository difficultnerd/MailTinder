import 'package:app/achievements.dart';
import 'package:app/api/models/stats.dart';
import 'package:app/api/models/swipe.dart';
import 'package:app/copy.dart';
import 'package:app/format.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import '../support/settings_harness.dart';

Stats _stats({List<Achievement> achievements = const []}) => Stats(
  emailsTriaged: 12431,
  sendersUnsubscribed: 87,
  unsubscribesConfirmed: 60,
  mailStoppedPerYear: 5400,
  achievements: achievements,
);

void main() {
  final unlockedAt = DateTime.utc(2026, 9, 20, 12);

  Future<SettingsHarness> open(WidgetTester tester, Stats stats) async {
    final h = SettingsHarness();
    h.api.stats = stats;
    await h.pump(tester);
    await h.openSettingsEntry(tester, Copy.stats);
    return h;
  }

  testWidgets('ST-02 AC1 Stats shows triaged, unsubscribed and confirmed', (
    tester,
  ) async {
    await open(tester, _stats());
    expect(find.text(Copy.emailsTriaged), findsOneWidget);
    expect(find.text('12,431'), findsOneWidget);
    expect(find.text(Copy.sendersUnsubscribed), findsOneWidget);
    expect(find.text('87'), findsOneWidget);
    expect(find.text(Copy.unsubscribesConfirmed), findsOneWidget);
    expect(find.text('60'), findsOneWidget);
  });

  testWidgets('ST-02 AC2 Stats shows the mail stopped total', (tester) async {
    await open(tester, _stats());
    expect(find.text('About 5,400 emails a year stopped'), findsOneWidget);
  });

  testWidgets('ST-02 AC3 achievements unlocked with dates and locked greyed', (
    tester,
  ) async {
    final handle = tester.ensureSemantics();
    tester.view.physicalSize = const Size(800, 2400);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.reset);
    await open(
      tester,
      _stats(
        achievements: [
          Achievement(
            achievementId: 'first_unsubscribe',
            unlockedAt: unlockedAt,
          ),
        ],
      ),
    );
    expect(find.text('First unsubscribe'), findsOneWidget);
    expect(find.text(formatDate(unlockedAt)), findsOneWidget);
    expect(
      find.bySemanticsLabel('100 senders silenced, locked'),
      findsOneWidget,
    );
    final opacity = tester.widget<Opacity>(
      find.ancestor(
        of: find.text('100 senders silenced'),
        matching: find.byType(Opacity),
      ),
    );
    expect(opacity.opacity, 0.4);
    handle.dispose();
  });

  testWidgets('GM-06 AC3 unlocked achievement appears in Stats', (
    tester,
  ) async {
    await open(
      tester,
      _stats(
        achievements: [
          Achievement(achievementId: 'cleared_1000', unlockedAt: unlockedAt),
          Achievement(achievementId: 'brand_new', unlockedAt: unlockedAt),
        ],
      ),
    );
    expect(find.text('1,000 cleared'), findsOneWidget);
    expect(find.text(Copy.achievementUnknown), findsOneWidget);
    expect(find.text(formatDate(unlockedAt)), findsNWidgets(2));
  });

  test(
    'achievementTitle covers every catalogue ID and returns null for unknown',
    () {
      for (final a in kAchievements) {
        expect(achievementTitle(a.id), a.title);
      }
      expect(achievementTitle('nope'), isNull);
      expect(kAchievements, hasLength(7));
    },
  );
}
