import 'package:flutter/material.dart';

import '../../api/models/feed.dart';
import 'bulk_badge.dart';

/// One card in focus (FD-01 AC1). All mail strings render through plain
/// [Text] widgets, never RichText/SelectableText/Html/Markdown, so hostile
/// input is output-encoded (ASVS V1.1.2, V3.2.2).
class CardView extends StatelessWidget {
  const CardView({super.key, required this.card, required this.mailboxAddress});

  final FeedCard card;

  /// The mailbox's email address (FD-02 AC2); empty when the mailbox is not in
  /// the session so the badge is omitted.
  final String mailboxAddress;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Card(
      clipBehavior: Clip.antiAlias,
      child: Padding(
        padding: const EdgeInsets.all(16),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          mainAxisSize: MainAxisSize.min,
          children: [
            Text(
              card.senderName,
              style: theme.textTheme.titleLarge,
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
            ),
            Text(
              card.senderAddress,
              style: theme.textTheme.bodyMedium,
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
            ),
            if (mailboxAddress.isNotEmpty)
              Text(
                mailboxAddress,
                style: theme.textTheme.labelMedium?.copyWith(
                  color: theme.colorScheme.primary,
                ),
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
              ),
            const SizedBox(height: 12),
            Text(
              card.subject,
              style: theme.textTheme.titleMedium,
              maxLines: 2,
              overflow: TextOverflow.ellipsis,
            ),
            const SizedBox(height: 8),
            Text(card.preview, maxLines: 6, overflow: TextOverflow.ellipsis),
            const SizedBox(height: 12),
            Row(
              children: [
                BulkBadge(score: card.bulkScore, reason: card.bulkReason),
                if (card.classifierId != null) ...[
                  const SizedBox(width: 12),
                  Flexible(
                    child: Text(
                      card.classifierId!,
                      style: theme.textTheme.bodySmall,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                    ),
                  ),
                ],
              ],
            ),
          ],
        ),
      ),
    );
  }
}
