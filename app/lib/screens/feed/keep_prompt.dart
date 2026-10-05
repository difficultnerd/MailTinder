import 'package:flutter/material.dart';

import '../../api/models/feed.dart';
import '../../copy.dart';

/// The keep-learning prompt (FL-04): inline on the card, quiet, with a "File"
/// button and a dismiss icon. Dismissing hides it for this card only and sends
/// nothing.
class KeepPrompt extends StatefulWidget {
  const KeepPrompt({super.key, required this.card, required this.onFile});

  final FeedCard card;
  final Future<void> Function() onFile;

  @override
  State<KeepPrompt> createState() => _KeepPromptState();
}

class _KeepPromptState extends State<KeepPrompt> {
  bool _dismissed = false;

  @override
  Widget build(BuildContext context) {
    final prompt = widget.card.keepPrompt;
    if (prompt == null || _dismissed) return const SizedBox.shrink();
    final theme = Theme.of(context);
    return Padding(
      padding: const EdgeInsets.only(top: 12),
      child: Row(
        children: [
          Expanded(
            child: Text(
              Copy.keepPrompt(prompt.name),
              style: theme.textTheme.bodySmall,
            ),
          ),
          Semantics(
            label: Copy.fileButton,
            button: true,
            child: TextButton(
              onPressed: widget.onFile,
              child: const Text(Copy.fileButton),
            ),
          ),
          Semantics(
            label: Copy.dismiss,
            button: true,
            child: IconButton(
              tooltip: Copy.dismiss,
              onPressed: () => setState(() => _dismissed = true),
              icon: const Icon(Icons.close),
            ),
          ),
        ],
      ),
    );
  }
}
