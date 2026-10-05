/// S7 `HistoryEntry` and the API-HIST-1 page.
library;

enum HistoryFilter {
  all('all'),
  unsubscribes('unsubscribes'),
  ruleActions('rule_actions'),
  filing('filing');

  const HistoryFilter(this.wire);

  final String wire;
}

class HistoryEntry {
  const HistoryEntry({
    required this.entryId,
    required this.at,
    required this.mailboxId,
    required this.senderDisplay,
    required this.action,
    required this.outcome,
    this.ruleId,
  });

  factory HistoryEntry.fromJson(Map<String, Object?> json) {
    return HistoryEntry(
      entryId: (json['entry_id'] as String?) ?? '',
      at: DateTime.parse((json['at'] as String?) ?? '').toUtc(),
      mailboxId: (json['mailbox_id'] as String?) ?? '',
      senderDisplay: (json['sender_display'] as String?) ?? '',
      action: (json['action'] as String?) ?? '',
      outcome: (json['outcome'] as String?) ?? '',
      ruleId: json['rule_id'] as String?,
    );
  }

  final String entryId;
  final DateTime at;
  final String mailboxId;
  final String senderDisplay;
  final String action;
  final String outcome;
  final String? ruleId;
}

class HistoryPage {
  const HistoryPage({required this.entries, required this.nextCursor});

  factory HistoryPage.fromJson(Map<String, Object?> json) {
    final raw = json['entries'];
    return HistoryPage(
      entries: raw is List<Object?>
          ? raw
                .whereType<Map<String, Object?>>()
                .map(HistoryEntry.fromJson)
                .toList(growable: false)
          : const <HistoryEntry>[],
      nextCursor: json['next_cursor'] as String?,
    );
  }

  final List<HistoryEntry> entries;
  final String? nextCursor;
}
