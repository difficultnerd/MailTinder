import 'dart:async';

import 'package:flutter/material.dart';

import '../../achievements.dart';
import '../../copy.dart';
import '../../state/swipe_controller.dart';

/// How long each celebration shows [DEFAULT].
const Duration kCelebrationDuration = Duration(seconds: 2);

/// A queue of short overlay messages: level complete, achievement unlocked,
/// boss defeated (GM-04 AC2, GM-06 AC3, GM-08 AC3). Memory only.
class CelebrationQueue extends ChangeNotifier {
  final List<String> _queue = [];

  /// The message showing now, or null.
  String? get current => _queue.isEmpty ? null : _queue.first;

  void add(String message) {
    _queue.add(message);
    notifyListeners();
  }

  void addLevelComplete(int oldYear, int newYear) =>
      add(Copy.levelComplete(oldYear, newYear));

  /// Queues the celebrations carried by an acked swipe.
  void onEvent(SwipeEvent e) {
    if (e is! SwipeAcked) return;
    for (final a in e.result.achievementsUnlocked) {
      final title = achievementTitle(a.achievementId);
      add(
        title == null
            ? Copy.achievementUnlocked('').trimRight()
            : Copy.achievementUnlocked(title),
      );
    }
    if (e.result.bossDefeated) add(Copy.bossDefeated(e.card.senderName));
  }

  void dismiss() {
    if (_queue.isEmpty) return;
    _queue.removeAt(0);
    notifyListeners();
  }
}

/// Shows the head of [queue] for [kCelebrationDuration] or until tapped.
class CelebrationOverlay extends StatefulWidget {
  const CelebrationOverlay({super.key, required this.queue});

  final CelebrationQueue queue;

  @override
  State<CelebrationOverlay> createState() => _CelebrationOverlayState();
}

class _CelebrationOverlayState extends State<CelebrationOverlay> {
  Timer? _timer;
  String? _shown;

  @override
  void dispose() {
    _timer?.cancel();
    super.dispose();
  }

  void _sync(String? current) {
    if (current == null) {
      _timer?.cancel();
      _shown = null;
      return;
    }
    // A new message (even with equal text) restarts the timer after a dismiss.
    if (_timer == null || !_timer!.isActive) {
      _shown = current;
      _timer = Timer(kCelebrationDuration, widget.queue.dismiss);
    }
  }

  @override
  Widget build(BuildContext context) {
    return ListenableBuilder(
      listenable: widget.queue,
      builder: (context, _) {
        final current = widget.queue.current;
        _sync(current);
        if (current == null) return const SizedBox.shrink();
        final scheme = Theme.of(context).colorScheme;
        return Align(
          alignment: Alignment.topCenter,
          child: Padding(
            padding: const EdgeInsets.only(top: 48, left: 16, right: 16),
            child: Semantics(
              liveRegion: true,
              label: current,
              excludeSemantics: true,
              child: GestureDetector(
                onTap: () {
                  _timer?.cancel();
                  widget.queue.dismiss();
                },
                child: Material(
                  color: scheme.primaryContainer,
                  elevation: 4,
                  borderRadius: BorderRadius.circular(12),
                  child: Padding(
                    padding: const EdgeInsets.all(16),
                    child: Text(
                      _shown ?? current,
                      style: TextStyle(color: scheme.onPrimaryContainer),
                    ),
                  ),
                ),
              ),
            ),
          ),
        );
      },
    );
  }
}
