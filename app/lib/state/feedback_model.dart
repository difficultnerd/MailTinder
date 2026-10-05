import 'package:flutter/foundation.dart';

import '../api/models/feed.dart';
import '../api/models/swipe.dart';
import '../platform/haptics.dart';
import '../platform/sound_player.dart';
import 'play_prefs.dart';

/// GM-02 AC3 [TUNABLE]: swipes within [kComboWindow] that show the combo.
const int kComboSwipes = 5;

/// GM-02 AC3 [TUNABLE].
const Duration kComboWindow = Duration(seconds: 10);

/// GM-02 AC3: confetti at every this many cleared.
const int kConfettiEvery = 100;

/// [DEFAULT]: a reject on a card scoring at least this plays the flush, until
/// the badge artwork exists.
const int kFlushScore = 80;

/// Which animation a swipe plays (GM-02 AC1).
enum CardEffect { flyRight, flyLeft, flyUp, flyDown, flush }

/// Drives the swipe effects (GM-02): which animation plays, optional sound and
/// haptics, the combo counter and the confetti trigger. Memory only; nothing
/// is persisted.
class FeedbackModel extends ChangeNotifier {
  FeedbackModel({
    required PlayPrefs prefs,
    required SoundPlayer sound,
    required Haptics haptics,
    DateTime Function()? now,
  }) : _prefs = prefs,
       _sound = sound,
       _haptics = haptics,
       _now = now ?? DateTime.now;

  final PlayPrefs _prefs;
  final SoundPlayer _sound;
  final Haptics _haptics;
  final DateTime Function() _now;

  final List<DateTime> _times = [];
  int _cleared = 0;
  int _milestone = 0;
  bool _confettiDue = false;

  /// The swipe count in the window, or null until [kComboSwipes] swipes fall
  /// within [kComboWindow].
  int? get combo => _times.length >= kComboSwipes ? _times.length : null;

  /// True once per crossing of a multiple of [kConfettiEvery] cleared.
  bool get confettiDue => _confettiDue;

  /// The cleared count that triggered [confettiDue].
  int get milestone => _milestone;

  void acknowledgeConfetti() {
    if (!_confettiDue) return;
    _confettiDue = false;
    notifyListeners();
  }

  /// Called at swipe time (optimistic): picks the effect, plays the sound and
  /// haptic, and updates the combo and confetti counters.
  CardEffect onSwipe(FeedCard card, SwipeKind kind) {
    final effect = switch (kind) {
      SwipeKind.keep => CardEffect.flyRight,
      SwipeKind.reject =>
        card.bulkScore >= kFlushScore ? CardEffect.flush : CardEffect.flyLeft,
      SwipeKind.file => CardEffect.flyUp,
      SwipeKind.skip => CardEffect.flyDown,
    };
    if (_prefs.soundsOn) {
      _sound.play(switch (effect) {
        CardEffect.flyRight => SwipeSound.keep,
        CardEffect.flyLeft => SwipeSound.reject,
        CardEffect.flyUp => SwipeSound.file,
        CardEffect.flyDown => SwipeSound.skip,
        CardEffect.flush => SwipeSound.flush,
      });
    }
    if (_haptics.supported) _haptics.tap();

    final now = _now();
    _times
      ..removeWhere((t) => now.difference(t) > kComboWindow)
      ..add(now);

    if (kind == SwipeKind.reject || kind == SwipeKind.file) {
      _cleared++;
      if (_cleared % kConfettiEvery == 0) {
        _confettiDue = true;
        _milestone = _cleared;
      }
    }
    notifyListeners();
    return effect;
  }
}
