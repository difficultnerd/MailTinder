/// S7 `Rule` (API-RULE-1, API-RULE-3).
library;

enum RuleKind {
  rejectList('reject_list'),
  blockPerson('block_person'),
  file('file');

  const RuleKind(this.wire);

  final String wire;

  static RuleKind? fromWire(String? wire) {
    for (final k in RuleKind.values) {
      if (k.wire == wire) {
        return k;
      }
    }
    return null;
  }
}

class RuleMatch {
  const RuleMatch({required this.senderAddress, this.listId});

  factory RuleMatch.fromJson(Map<String, Object?> json) {
    return RuleMatch(
      senderAddress: (json['sender_address'] as String?) ?? '',
      listId: json['list_id'] as String?,
    );
  }

  final String senderAddress;
  final String? listId;
}

class Rule {
  const Rule({
    required this.ruleId,
    required this.kind,
    required this.match,
    required this.categoryId,
    required this.enabled,
    required this.createdAt,
    required this.timesApplied,
    required this.yearlyRate,
  });

  /// Throws [FormatException] for an unknown kind rather than guessing.
  factory Rule.fromJson(Map<String, Object?> json) {
    final kind = RuleKind.fromWire(json['kind'] as String?);
    if (kind == null) {
      throw const FormatException('unknown rule kind');
    }
    final match = json['match'];
    return Rule(
      ruleId: (json['rule_id'] as String?) ?? '',
      kind: kind,
      match: match is Map<String, Object?>
          ? RuleMatch.fromJson(match)
          : const RuleMatch(senderAddress: ''),
      categoryId: json['category_id'] as String?,
      enabled: (json['enabled'] as bool?) ?? false,
      createdAt: DateTime.parse((json['created_at'] as String?) ?? '').toUtc(),
      timesApplied: (json['times_applied'] as num?)?.toInt() ?? 0,
      yearlyRate: (json['yearly_rate'] as num?)?.toInt(),
    );
  }

  final String ruleId;
  final RuleKind kind;
  final RuleMatch match;
  final String? categoryId;
  final bool enabled;
  final DateTime createdAt;
  final int timesApplied;

  /// Null means unknown, never zero (GM-05 AC3).
  final int? yearlyRate;

  Rule copyWith({bool? enabled}) => Rule(
    ruleId: ruleId,
    kind: kind,
    match: match,
    categoryId: categoryId,
    enabled: enabled ?? this.enabled,
    createdAt: createdAt,
    timesApplied: timesApplied,
    yearlyRate: yearlyRate,
  );
}
