import 'package:flutter/material.dart';

import '../../copy.dart';
import '../../state/round_tracker.dart';

/// The end-of-round card (GM-03 AC1): the five totals and "Keep going".
class RoundCard extends StatelessWidget {
  const RoundCard({super.key, required this.totals, required this.onDismiss});

  final RoundTotals totals;
  final VoidCallback onDismiss;

  @override
  Widget build(BuildContext context) {
    return ColoredBox(
      color: Colors.black54,
      child: Center(
        child: Semantics(
          scopesRoute: true,
          explicitChildNodes: true,
          label: Copy.roundOver,
          child: Card(
            margin: const EdgeInsets.all(24),
            child: Padding(
              padding: const EdgeInsets.all(24),
              child: Column(
                mainAxisSize: MainAxisSize.min,
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(
                    Copy.roundOver,
                    style: Theme.of(context).textTheme.titleLarge,
                  ),
                  const SizedBox(height: 16),
                  _row(Copy.roundCleared, totals.cleared),
                  _row(Copy.roundKept, totals.kept),
                  _row(Copy.roundFiled, totals.filed),
                  _row(Copy.roundUnsubscribed, totals.unsubscribed),
                  _row(Copy.roundBlocked, totals.blocked),
                  const SizedBox(height: 16),
                  Align(
                    alignment: Alignment.centerRight,
                    child: FilledButton(
                      onPressed: onDismiss,
                      child: const Text(Copy.keepGoing),
                    ),
                  ),
                ],
              ),
            ),
          ),
        ),
      ),
    );
  }

  Widget _row(String label, int n) => Semantics(
    label: '$label: $n',
    excludeSemantics: true,
    child: Padding(
      padding: const EdgeInsets.symmetric(vertical: 2),
      child: Row(
        mainAxisAlignment: MainAxisAlignment.spaceBetween,
        children: [Text(label), Text('$n')],
      ),
    ),
  );
}
