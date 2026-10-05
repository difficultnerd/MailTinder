import 'package:flutter/material.dart';

import '../../copy.dart';
import '../../state/blitz_model.dart';

/// The results card shown when a Blitz round ends (GM-07 AC1).
class BlitzResultsCard extends StatelessWidget {
  const BlitzResultsCard({
    super.key,
    required this.result,
    required this.onDone,
  });

  final BlitzResult result;
  final VoidCallback onDone;

  @override
  Widget build(BuildContext context) {
    return ColoredBox(
      color: Colors.black54,
      child: Center(
        child: Semantics(
          scopesRoute: true,
          explicitChildNodes: true,
          label: Copy.blitzOver,
          child: Card(
            margin: const EdgeInsets.all(24),
            child: Padding(
              padding: const EdgeInsets.all(24),
              child: Column(
                mainAxisSize: MainAxisSize.min,
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(
                    Copy.blitzOver,
                    style: Theme.of(context).textTheme.titleLarge,
                  ),
                  const SizedBox(height: 16),
                  _row(Copy.blitzScoreRow, result.score),
                  _row(Copy.blitzCleared, result.cleared),
                  _row(Copy.blitzKept, result.kept),
                  _row(Copy.blitzFiled, result.filed),
                  const SizedBox(height: 16),
                  Align(
                    alignment: Alignment.centerRight,
                    child: FilledButton(
                      onPressed: onDone,
                      child: const Text(Copy.blitzDone),
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
        mainAxisSize: MainAxisSize.min,
        children: [
          SizedBox(width: 120, child: Text(label)),
          Text('$n'),
        ],
      ),
    ),
  );
}
