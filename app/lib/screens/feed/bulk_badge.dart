import 'package:flutter/material.dart';

import '../../copy.dart';

/// The bulk score badge. Tapping it shows the reason in a bottom sheet
/// (S9 Feed "Bulk badge tap"). XC-03: labelled as a button.
class BulkBadge extends StatelessWidget {
  const BulkBadge({super.key, required this.score, required this.reason});

  final int score;
  final String reason;

  @override
  Widget build(BuildContext context) {
    return Semantics(
      label: Copy.bulkBadgeSemantics(score),
      button: true,
      child: InkWell(
        onTap: () => _showReason(context),
        borderRadius: BorderRadius.circular(16),
        child: Chip(
          label: Text(Copy.bulkBadge(score)),
          visualDensity: VisualDensity.compact,
        ),
      ),
    );
  }

  void _showReason(BuildContext context) {
    showModalBottomSheet<void>(
      context: context,
      builder: (sheetContext) => SafeArea(
        child: Padding(
          padding: const EdgeInsets.all(24),
          child: Text(reason, textAlign: TextAlign.center),
        ),
      ),
    );
  }
}
