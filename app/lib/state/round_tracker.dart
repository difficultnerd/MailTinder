import 'package:flutter/foundation.dart';

import '../api/models/swipe.dart';
import 'swipe_controller.dart';

/// GM-03 AC1 [TUNABLE]: a round card is due after this many swipes.
const int kRoundSwipes = 50;

class RoundTotals {
  int cleared = 0, kept = 0, filed = 0, unsubscribed = 0, blocked = 0;
}

/// Counts the current round (GM-03). Memory only: nothing is stored, so a new
/// app instance starts at zero (GM-03 AC2).
class RoundTracker extends ChangeNotifier {
  RoundTotals _totals = RoundTotals();
  int _swipesSinceCard = 0;
  bool _pending = false;

  RoundTotals get totals => _totals;
  int get swipesSinceCard => _swipesSinceCard;

  /// True after [kRoundSwipes] swipes, or when the divider or a Feed exit
  /// marked the round over. A round with no swipes never shows a card.
  bool get cardDue =>
      _swipesSinceCard > 0 && (_pending || _swipesSinceCard >= kRoundSwipes);

  void onEvent(SwipeEvent e) {
    switch (e) {
      case SwipeAcked(:final kind, :final result):
        _apply(kind, result, 1);
        _swipesSinceCard++;
      case SwipeUndone(:final kind, :final result):
        _apply(kind, result, -1);
        _swipesSinceCard++;
      case BlockAccepted():
        _totals.blocked++;
    }
    notifyListeners();
  }

  void _apply(SwipeKind kind, SwipeResult result, int sign) {
    final t = _totals;
    switch (kind) {
      case SwipeKind.keep:
        t.kept = _floor(t.kept + sign);
      case SwipeKind.reject:
        t.cleared = _floor(t.cleared + sign);
      case SwipeKind.file:
        t.cleared = _floor(t.cleared + sign);
        t.filed = _floor(t.filed + sign);
      case SwipeKind.skip:
        break;
    }
    if (result.outcome == 'trashed_unsubscribe_queued') {
      t.unsubscribed = _floor(t.unsubscribed + sign);
    }
  }

  int _floor(int n) => n < 0 ? 0 : n;

  /// The Feed tab stopped being selected: the card is due when it returns.
  void markLeftFeed() => _mark();

  /// The up-to-date divider became the current item.
  void markDividerReached() => _mark();

  void _mark() {
    if (_swipesSinceCard == 0) return;
    _pending = true;
    notifyListeners();
  }

  /// Returns the totals and starts a new round.
  RoundTotals takeCard() {
    final taken = _totals;
    _totals = RoundTotals();
    _swipesSinceCard = 0;
    _pending = false;
    notifyListeners();
    return taken;
  }
}
