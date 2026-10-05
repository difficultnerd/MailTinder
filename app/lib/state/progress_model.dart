import 'package:flutter/foundation.dart';

import '../api/api_client.dart';
import '../api/models/progress.dart';

/// GM-01 AC2 [TUNABLE]: reload the meter after this many acked swipes.
const int kProgressEverySwipes = 10;

/// Drives the inbox meter, the level banner and boss health bars (GM-01,
/// GM-04, GM-08). Everything here is in memory only.
class ProgressModel extends ChangeNotifier {
  ProgressModel({required ApiClient api}) : _api = api;

  final ApiClient _api;

  int? _inboxCount;
  int? _startCount;
  bool _partial = false;
  Level? _level;
  Level? _completed;
  int _acks = 0;
  final Map<String, int> _bossFirstSeen = {};

  int? get inboxCount => _inboxCount;

  /// The count from the first successful load this session.
  int? get startCount => _startCount;

  /// True when a mailbox did not answer, so the count is a partial total.
  bool get partial => _partial;

  Level? get level => _level;

  /// The level that was just completed (the year moved to an older one), or
  /// null. Returns it once.
  Level? takeCompletedLevel() {
    final done = _completed;
    _completed = null;
    return done;
  }

  Future<void> load() async {
    final Progress p;
    try {
      p = await _api.getProgress();
    } on Object {
      return; // keep what is showing
    }
    _startCount ??= p.inboxCount;
    _inboxCount = p.inboxCount;
    _partial = p.mailboxErrors.isNotEmpty;
    final old = _level;
    final next = p.level;
    if (old != null && next != null && next.year < old.year) {
      _completed = old;
    }
    _level = next;
    notifyListeners();
  }

  /// Counts an acked swipe; reloads every [kProgressEverySwipes] acks.
  void onSwipeAcked() {
    _acks++;
    if (_acks % kProgressEverySwipes == 0) {
      load();
    }
  }

  /// Boss health as a 0..1 fraction of the first `remaining` seen for
  /// [senderAddress] this session (GM-08 AC2).
  double bossHealth(String senderAddress, int remaining) {
    final first = _bossFirstSeen.putIfAbsent(senderAddress, () => remaining);
    if (first <= 0) return 0;
    return (remaining / first).clamp(0.0, 1.0);
  }
}
