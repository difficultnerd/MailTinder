import 'dart:async';

import 'package:flutter/widgets.dart';

import '../api/models/feed.dart';
import '../api/models/swipe.dart';
import 'feed_model.dart';
import 'swipe_controller.dart';

/// GM-07 AC1: how long a Blitz round lasts.
const Duration kBlitzLength = Duration(seconds: 60);

enum BlitzStatus { idle, running, paused, ended }

class BlitzResult {
  const BlitzResult({
    required this.score,
    required this.cleared,
    required this.kept,
    required this.filed,
  });

  final int score, cleared, kept, filed;
}

/// A 60-second round on the Feed (S9 section 3.1, GM-07). Blitz only filters
/// personal cards, holds block prompts and counts; swipes, the unsubscribe
/// toast and undo are untouched. Memory only: nothing about a round outlives
/// its results card (GM-07 AC5).
class BlitzModel extends ChangeNotifier {
  BlitzModel({required FeedModel feed, required SwipeController swipes})
    : _feed = feed,
      _swipes = swipes;

  final FeedModel _feed;
  final SwipeController _swipes;

  BlitzStatus _status = BlitzStatus.idle;
  Duration _remaining = kBlitzLength;
  int _score = 0, _cleared = 0, _kept = 0, _filed = 0;
  BlitzResult? _result;
  Timer? _timer;
  StreamSubscription<SwipeEvent>? _events;
  AppLifecycleListener? _lifecycle;
  bool _disposed = false;

  BlitzStatus get status => _status;
  Duration get remaining => _remaining;
  int get score => _score;
  BlitzResult? get result => _result;

  void start() {
    if (_status != BlitzStatus.idle) return;
    _status = BlitzStatus.running;
    _remaining = kBlitzLength;
    _score = _cleared = _kept = _filed = 0;
    _feed.setHiddenFilter((c) => c.messageClass == MessageClass.personal);
    _swipes.holdPrompts = true;
    _events = _swipes.events.listen(_onEvent);
    _lifecycle = AppLifecycleListener(
      onHide: pause,
      onShow: resume,
      onResume: resume,
    );
    _startTimer();
    notifyListeners();
  }

  void _startTimer() {
    _timer?.cancel();
    _timer = Timer.periodic(const Duration(seconds: 1), (_) => _tick());
  }

  void _tick() {
    if (_status != BlitzStatus.running) return;
    _remaining -= const Duration(seconds: 1);
    if (_remaining <= Duration.zero) {
      _remaining = Duration.zero;
      end();
      return;
    }
    notifyListeners();
  }

  void _onEvent(SwipeEvent e) {
    if (_status != BlitzStatus.running && _status != BlitzStatus.paused) {
      return;
    }
    switch (e) {
      case SwipeAcked(:final kind):
        _count(kind, 1);
      case SwipeUndone(:final kind):
        _count(kind, -1);
      case BlockAccepted():
        return;
    }
    notifyListeners();
  }

  void _count(SwipeKind kind, int sign) {
    if (kind == SwipeKind.skip) return;
    _score = _floor(_score + sign);
    switch (kind) {
      case SwipeKind.keep:
        _kept = _floor(_kept + sign);
      case SwipeKind.reject:
        _cleared = _floor(_cleared + sign);
      case SwipeKind.file:
        _cleared = _floor(_cleared + sign);
        _filed = _floor(_filed + sign);
      case SwipeKind.skip:
        break;
    }
  }

  int _floor(int n) => n < 0 ? 0 : n;

  /// The tab was hidden: stop the clock (S9 "interrupted").
  void pause() {
    if (_status != BlitzStatus.running) return;
    _status = BlitzStatus.paused;
    notifyListeners();
  }

  void resume() {
    if (_status != BlitzStatus.paused) return;
    _status = BlitzStatus.running;
    notifyListeners();
  }

  /// End round, or the timer reaching zero.
  void end() {
    if (_status != BlitzStatus.running && _status != BlitzStatus.paused) {
      return;
    }
    _stopWatching();
    _status = BlitzStatus.ended;
    _feed.setHiddenFilter(null);
    _swipes.holdPrompts = false;
    _result = BlitzResult(
      score: _score,
      cleared: _cleared,
      kept: _kept,
      filed: _filed,
    );
    notifyListeners();
  }

  void _stopWatching() {
    _timer?.cancel();
    _timer = null;
    _events?.cancel();
    _events = null;
    _lifecycle?.dispose();
    _lifecycle = null;
  }

  /// Closes the results card, then shows each held block prompt in turn.
  Future<void> dismissResults() async {
    if (_status != BlitzStatus.ended) return;
    final held = _swipes.takeHeldPrompts();
    _result = null;
    _score = _cleared = _kept = _filed = 0;
    _remaining = kBlitzLength;
    _status = BlitzStatus.idle;
    notifyListeners();
    if (held.isNotEmpty) await _swipes.presentPrompts(held);
  }

  @override
  void dispose() {
    _disposed = true;
    if (_status == BlitzStatus.running || _status == BlitzStatus.paused) {
      end();
    }
    _stopWatching();
    super.dispose();
  }

  @override
  void notifyListeners() {
    if (_disposed) return;
    super.notifyListeners();
  }
}
