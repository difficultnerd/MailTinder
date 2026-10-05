import 'package:flutter/material.dart';

import '../../copy.dart';

/// The block prompt dialog (PB-01, S9 section 3): "You've rejected `name` 3
/// times. Block them?" with Block preselected (autofocus, filled) and Not now.
/// Returns true for Block, false for Not now.
class BlockPromptDialog extends StatelessWidget {
  const BlockPromptDialog({super.key, required this.senderName});

  final String senderName;

  @override
  Widget build(BuildContext context) {
    return AlertDialog(
      title: Text(Copy.blockQuestion(senderName)),
      actions: [
        TextButton(
          onPressed: () => Navigator.of(context).pop(false),
          child: const Text(Copy.notNow),
        ),
        FilledButton(
          autofocus: true,
          onPressed: () => Navigator.of(context).pop(true),
          child: const Text(Copy.block),
        ),
      ],
    );
  }
}
