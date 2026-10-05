import 'package:flutter/material.dart';

import '../../copy.dart';
import '../../state/blitz_model.dart';

/// The Blitz button (idle) or the running bar with timer, score and End round
/// (S9 section 3.1).
class BlitzBar extends StatelessWidget {
  const BlitzBar({super.key, required this.model});

  final BlitzModel model;

  @override
  Widget build(BuildContext context) {
    return ListenableBuilder(
      listenable: model,
      builder: (context, _) {
        final running =
            model.status == BlitzStatus.running ||
            model.status == BlitzStatus.paused;
        if (!running) {
          if (model.status == BlitzStatus.ended) return const SizedBox.shrink();
          return Align(
            alignment: Alignment.centerRight,
            child: Padding(
              padding: const EdgeInsets.symmetric(horizontal: 16),
              child: Semantics(
                label: Copy.blitzSemantics,
                button: true,
                excludeSemantics: true,
                child: TextButton.icon(
                  onPressed: model.start,
                  icon: const Icon(Icons.bolt),
                  label: const Text(Copy.blitz),
                ),
              ),
            ),
          );
        }
        final timer = Copy.blitzTimer(model.remaining);
        final score = Copy.blitzScore(model.score);
        return Padding(
          padding: const EdgeInsets.symmetric(horizontal: 16),
          child: Row(
            children: [
              Semantics(
                label: timer,
                excludeSemantics: true,
                child: Text(timer, key: const ValueKey('blitz-timer')),
              ),
              const SizedBox(width: 16),
              Semantics(
                label: score,
                excludeSemantics: true,
                child: Text(score, key: const ValueKey('blitz-score')),
              ),
              const Spacer(),
              Semantics(
                label: Copy.endRound,
                button: true,
                excludeSemantics: true,
                child: TextButton(
                  onPressed: model.end,
                  child: const Text(Copy.endRound),
                ),
              ),
            ],
          ),
        );
      },
    );
  }
}
