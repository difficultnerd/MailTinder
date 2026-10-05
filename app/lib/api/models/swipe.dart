/// S7 Swipe models for API-SW-1 (`POST /swipes`), API-SW-2
/// (`POST /swipes/undo`), API-RULE-2 (`POST /rules`) and API-RULE-5
/// (`POST /block-prompts/decline`).
library;

import 'feed.dart';

/// The four swipe directions (S7 `action` wire value).
enum SwipeKind {
  keep,
  skip,
  reject,
  file;

  String get wire => switch (this) {
    SwipeKind.keep => 'keep',
    SwipeKind.skip => 'skip',
    SwipeKind.reject => 'reject',
    SwipeKind.file => 'file',
  };
}

/// A single API-SW-1 request body.
class SwipeRequest {
  const SwipeRequest({
    required this.mailboxId,
    required this.messageId,
    required this.classificationToken,
    required this.kind,
    this.categoryId,
    this.newCategoryName,
  });

  final String mailboxId;
  final String messageId;
  final String classificationToken;
  final SwipeKind kind;

  /// File only: exactly one of [categoryId] or [newCategoryName] is set.
  final String? categoryId;
  final String? newCategoryName;

  Map<String, Object?> toJson() => {
    'mailbox_id': mailboxId,
    'message_id': messageId,
    'action': kind.wire,
    'category_id': categoryId,
    'new_category_name': newCategoryName,
    'classification_token': classificationToken,
  };
}

/// The API-SW-1 response.
class SwipeResult {
  const SwipeResult({
    required this.outcome,
    required this.undoToken,
    required this.prompts,
    required this.achievementsUnlocked,
    required this.bossDefeated,
    this.unsubscribeDueAt,
    this.filedCategory,
  });

  factory SwipeResult.fromJson(Map<String, Object?> json) {
    final dueAt = json['unsubscribe_due_at'] as String?;
    final filed = json['filed_category'];
    return SwipeResult(
      outcome: (json['outcome'] as String?) ?? '',
      unsubscribeDueAt: dueAt == null ? null : DateTime.parse(dueAt).toUtc(),
      filedCategory: filed is Map<String, Object?>
          ? CategoryRef.fromJson(filed)
          : null,
      undoToken: (json['undo_token'] as String?) ?? '',
      prompts: _blockPrompts(json['prompts']),
      achievementsUnlocked: _achievements(json['achievements_unlocked']),
      bossDefeated: (json['boss_defeated'] as bool?) ?? false,
    );
  }

  /// S7 outcome string, kept as received.
  final String outcome;
  final DateTime? unsubscribeDueAt;
  final CategoryRef? filedCategory;
  final String undoToken;
  final List<BlockPrompt> prompts;
  final List<Achievement> achievementsUnlocked;
  final bool bossDefeated;
}

/// A `block_person` prompt returned with a swipe (PB-01).
class BlockPrompt {
  const BlockPrompt({required this.promptRef, required this.senderName});

  factory BlockPrompt.fromJson(Map<String, Object?> json) {
    return BlockPrompt(
      promptRef: (json['prompt_ref'] as String?) ?? '',
      senderName: (json['sender_name'] as String?) ?? '',
    );
  }

  final String promptRef;
  final String senderName;
}

/// An achievement unlocked by a swipe (GM-06).
class Achievement {
  const Achievement({required this.achievementId, required this.unlockedAt});

  factory Achievement.fromJson(Map<String, Object?> json) {
    return Achievement(
      achievementId: (json['achievement_id'] as String?) ?? '',
      unlockedAt: DateTime.parse(
        (json['unlocked_at'] as String?) ?? '',
      ).toUtc(),
    );
  }

  final String achievementId;
  final DateTime unlockedAt;
}

/// The API-SW-2 response.
class UndoResult {
  const UndoResult({
    required this.restored,
    required this.unsubscribeAlreadySent,
  });

  factory UndoResult.fromJson(Map<String, Object?> json) {
    return UndoResult(
      restored: (json['restored'] as bool?) ?? false,
      unsubscribeAlreadySent:
          (json['unsubscribe_already_sent'] as bool?) ?? false,
    );
  }

  final bool restored;
  final bool unsubscribeAlreadySent;
}

List<BlockPrompt> _blockPrompts(Object? raw) {
  if (raw is! List<Object?>) return const <BlockPrompt>[];
  return raw
      .whereType<Map<String, Object?>>()
      .map(BlockPrompt.fromJson)
      .toList(growable: false);
}

List<Achievement> _achievements(Object? raw) {
  if (raw is! List<Object?>) return const <Achievement>[];
  return raw
      .whereType<Map<String, Object?>>()
      .map(Achievement.fromJson)
      .toList(growable: false);
}
