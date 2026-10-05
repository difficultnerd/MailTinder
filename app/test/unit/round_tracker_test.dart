import 'package:app/api/models/swipe.dart';
import 'package:app/state/round_tracker.dart';
import 'package:app/state/swipe_controller.dart';
import 'package:flutter_test/flutter_test.dart';

import '../support/cards.dart';

SwipeResult _r(String outcome) => SwipeResult(
  outcome: outcome,
  undoToken: 'u',
  prompts: const [],
  achievementsUnlocked: const [],
  bossDefeated: false,
);

void main() {
  test('round tracker counts acks by kind', () {
    final t = RoundTracker();
    final card = buildCard();
    t.onEvent(SwipeAcked(card, SwipeKind.reject, _r('trashed')));
    t.onEvent(
      SwipeAcked(card, SwipeKind.reject, _r('trashed_unsubscribe_queued')),
    );
    t.onEvent(SwipeAcked(card, SwipeKind.keep, _r('kept')));
    t.onEvent(SwipeAcked(card, SwipeKind.file, _r('filed')));
    t.onEvent(BlockAccepted('x'));
    final s = t.totals;
    expect(
      [s.cleared, s.kept, s.filed, s.unsubscribed, s.blocked],
      [3, 1, 1, 1, 1],
    );
    expect(t.swipesSinceCard, 4);
  });

  test('undo reverses round counts', () {
    final t = RoundTracker();
    final card = buildCard();
    final r = _r('trashed_unsubscribe_queued');
    t.onEvent(SwipeAcked(card, SwipeKind.reject, r));
    t.onEvent(SwipeUndone(card, SwipeKind.reject, r));
    final s = t.totals;
    expect([s.cleared, s.unsubscribed], [0, 0]);
    expect(t.swipesSinceCard, 2);
  });

  test('round card is due after 50 swipes and takeCard starts a new round', () {
    final t = RoundTracker();
    final card = buildCard();
    for (var i = 0; i < kRoundSwipes; i++) {
      expect(t.cardDue, isFalse);
      t.onEvent(SwipeAcked(card, SwipeKind.keep, _r('kept')));
    }
    expect(t.cardDue, isTrue);
    expect(t.takeCard().kept, kRoundSwipes);
    expect(t.cardDue, isFalse);
    expect(t.totals.kept, 0);
  });

  test('leaving the Feed or the divider with no swipes shows no card', () {
    final t = RoundTracker();
    t.markLeftFeed();
    t.markDividerReached();
    t.onEvent(SwipeAcked(buildCard(), SwipeKind.keep, _r('kept')));
    expect(t.cardDue, isFalse);
  });
}
