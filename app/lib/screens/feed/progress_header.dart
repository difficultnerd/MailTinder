import 'package:flutter/material.dart';

import '../../api/models/feed.dart';
import '../../copy.dart';
import '../../state/progress_model.dart';

/// The inbox meter, level banner and boss banner at the top of the Feed
/// (S9 section 3, GM-01, GM-04, GM-08).
class ProgressHeader extends StatelessWidget {
  const ProgressHeader({super.key, required this.progress, this.current});

  final ProgressModel progress;

  /// The card in focus, for the boss banner.
  final FeedCard? current;

  @override
  Widget build(BuildContext context) {
    return ListenableBuilder(
      listenable: progress,
      builder: (context, _) {
        final count = progress.inboxCount;
        final level = progress.level;
        final boss = current?.boss;
        final card = current;
        return Padding(
          padding: const EdgeInsets.fromLTRB(16, 8, 16, 0),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              if (count != null) _meter(context, count),
              if (level != null)
                Semantics(
                  label: Copy.level(level.year, level.remaining),
                  excludeSemantics: true,
                  child: Text(Copy.level(level.year, level.remaining)),
                ),
              if (boss != null && card != null) _boss(card, boss),
            ],
          ),
        );
      },
    );
  }

  Widget _meter(BuildContext context, int count) {
    final change = count - (progress.startCount ?? count);
    final text = Copy.meter(count, change);
    final style = Theme.of(context).textTheme.titleMedium;
    if (!progress.partial) {
      return Semantics(
        label: text,
        excludeSemantics: true,
        child: Text(text, style: style),
      );
    }
    return Semantics(
      label: '$text. ${Copy.meterPartial}',
      excludeSemantics: true,
      child: Text('$text*', style: style),
    );
  }

  Widget _boss(FeedCard card, BossInfo boss) {
    final label = Copy.bossLabel(card.senderName, boss.remaining);
    return Semantics(
      label: label,
      excludeSemantics: true,
      child: Padding(
        padding: const EdgeInsets.only(top: 4),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text(card.senderName),
            LinearProgressIndicator(
              value: progress.bossHealth(card.senderAddress, boss.remaining),
            ),
          ],
        ),
      ),
    );
  }
}
