import 'package:flutter/material.dart';

import '../../copy.dart';

/// The up-to-date divider card, shown once when the API reports
/// `phase_changed` (FD-03 AC2).
class DividerCard extends StatelessWidget {
  const DividerCard({super.key, required this.onContinue});

  final VoidCallback onContinue;

  @override
  Widget build(BuildContext context) {
    return Card(
      child: Padding(
        padding: const EdgeInsets.all(24),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            const Text(Copy.upToDate, textAlign: TextAlign.center),
            const SizedBox(height: 16),
            ElevatedButton(
              onPressed: onContinue,
              child: const Text(Copy.continueLabel),
            ),
          ],
        ),
      ),
    );
  }
}
