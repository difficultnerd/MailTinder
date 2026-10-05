/// S7 Card / FeedPage models for API-FEED-1 (`POST /api/v1/feed/next`).
///
/// `class` is a Dart keyword, so the wire `class` field maps to
/// [FeedCard.messageClass].
library;

enum MessageClass {
  list,
  bulkNoHeader,
  notice,
  personal,
  suspect;

  /// Parses the S7 `class` wire value. Unknown values throw rather than
  /// defaulting, so a forward-incompatible server is never mis-classified.
  static MessageClass fromWire(String? raw) {
    return switch (raw) {
      'list' => MessageClass.list,
      'bulk_no_header' => MessageClass.bulkNoHeader,
      'notice' => MessageClass.notice,
      'personal' => MessageClass.personal,
      'suspect' => MessageClass.suspect,
      _ => throw FormatException('Unknown message class: $raw'),
    };
  }
}

enum SuggestionConfidence { learned, suggested, none }

/// A filing suggestion (S7 `Suggestion`).
class Suggestion {
  const Suggestion({
    required this.categoryId,
    required this.name,
    required this.alternates,
    required this.confidence,
  });

  factory Suggestion.fromJson(Map<String, Object?> json) {
    final rawConfidence = json['confidence'] as String?;
    final confidence = switch (rawConfidence) {
      'learned' => SuggestionConfidence.learned,
      'suggested' => SuggestionConfidence.suggested,
      'none' => SuggestionConfidence.none,
      _ => throw FormatException(
        'Unknown suggestion confidence: $rawConfidence',
      ),
    };
    return Suggestion(
      categoryId: json['category_id'] as String?,
      name: json['name'] as String?,
      alternates: _categoryRefs(json['alternates']),
      confidence: confidence,
    );
  }

  final String? categoryId;
  final String? name;
  final List<CategoryRef> alternates;
  final SuggestionConfidence confidence;
}

/// A category reference (S7 `CategoryRef`).
class CategoryRef {
  const CategoryRef({required this.categoryId, required this.name});

  factory CategoryRef.fromJson(Map<String, Object?> json) {
    return CategoryRef(
      categoryId: (json['category_id'] as String?) ?? '',
      name: (json['name'] as String?) ?? '',
    );
  }

  final String categoryId;
  final String name;
}

/// A boss-sender health bar (S7 `Card.boss`).
class BossInfo {
  const BossInfo({required this.remaining});

  factory BossInfo.fromJson(Map<String, Object?> json) {
    return BossInfo(remaining: (json['remaining'] as num?)?.toInt() ?? 0);
  }

  final int remaining;
}

/// One card shown in the Feed. Named FeedCard because Flutter already has a
/// Material `Card` widget.
class FeedCard {
  const FeedCard({
    required this.mailboxId,
    required this.messageId,
    required this.senderName,
    required this.senderAddress,
    required this.subject,
    required this.preview,
    required this.bulkReason,
    required this.receivedAt,
    required this.bulkScore,
    required this.skipCount,
    required this.messageClass,
    required this.hasOneClick,
    required this.suggestion,
    required this.keepPrompt,
    required this.boss,
    required this.providerWebUrl,
    required this.classificationToken,
    this.classifierId,
  });

  factory FeedCard.fromJson(Map<String, Object?> json) {
    final rawClass = json['class'] as String?;
    final messageClass = switch (rawClass) {
      'list' => MessageClass.list,
      'bulk_no_header' => MessageClass.bulkNoHeader,
      'notice' => MessageClass.notice,
      'personal' => MessageClass.personal,
      'suspect' => MessageClass.suspect,
      _ => throw FormatException('Unknown message class: $rawClass'),
    };

    final suggestionJson = json['suggestion'];
    final keepPromptJson = json['keep_prompt'];
    final bossJson = json['boss'];

    return FeedCard(
      mailboxId: (json['mailbox_id'] as String?) ?? '',
      messageId: (json['message_id'] as String?) ?? '',
      senderName: (json['sender_name'] as String?) ?? '',
      senderAddress: (json['sender_address'] as String?) ?? '',
      subject: (json['subject'] as String?) ?? '',
      preview: (json['preview'] as String?) ?? '',
      bulkReason: (json['bulk_reason'] as String?) ?? '',
      receivedAt: DateTime.parse(
        (json['received_at'] as String?) ?? '',
      ).toUtc(),
      bulkScore: (json['bulk_score'] as num?)?.toInt() ?? 0,
      skipCount: (json['skip_count'] as num?)?.toInt() ?? 0,
      messageClass: messageClass,
      hasOneClick: (json['has_one_click'] as bool?) ?? false,
      suggestion: suggestionJson is Map<String, Object?>
          ? Suggestion.fromJson(suggestionJson)
          : null,
      keepPrompt: keepPromptJson is Map<String, Object?>
          ? CategoryRef.fromJson(keepPromptJson)
          : null,
      boss: bossJson is Map<String, Object?>
          ? BossInfo.fromJson(bossJson)
          : null,
      providerWebUrl: Uri.parse((json['provider_web_url'] as String?) ?? ''),
      classificationToken: (json['classification_token'] as String?) ?? '',
      classifierId: json['classifier_id'] as String?,
    );
  }

  final String mailboxId;
  final String messageId;
  final String senderName;
  final String senderAddress;
  final String subject;
  final String preview;
  final String bulkReason;
  final DateTime receivedAt;
  final int bulkScore;
  final int skipCount;
  final MessageClass messageClass; // wire field "class"
  final bool hasOneClick;
  final Suggestion? suggestion;
  final CategoryRef? keepPrompt;
  final BossInfo? boss;
  final Uri providerWebUrl;
  final String classificationToken;
  final String? classifierId; // admins only
}

/// A per-mailbox error returned alongside cards (S7 `MailboxError`).
class MailboxError {
  const MailboxError({required this.mailboxId, required this.code});

  factory MailboxError.fromJson(Map<String, Object?> json) {
    return MailboxError(
      mailboxId: (json['mailbox_id'] as String?) ?? '',
      code: (json['code'] as String?) ?? '',
    );
  }

  final String mailboxId;
  final String code;
}

/// One page of cards from API-FEED-1 (S7 `FeedPage`).
class FeedPage {
  const FeedPage({
    required this.cards,
    required this.nextCursor,
    required this.phase,
    required this.phaseChanged,
    required this.mailboxErrors,
    required this.ruleActionsApplied,
  });

  factory FeedPage.fromJson(Map<String, Object?> json) {
    return FeedPage(
      cards: _cards(json['cards']),
      nextCursor: json['next_cursor'] as String?,
      phase: (json['phase'] as String?) ?? '',
      phaseChanged: (json['phase_changed'] as bool?) ?? false,
      mailboxErrors: _mailboxErrors(json['mailbox_errors']),
      ruleActionsApplied: (json['rule_actions_applied'] as num?)?.toInt() ?? 0,
    );
  }

  final List<FeedCard> cards;
  final String? nextCursor;
  final String phase;
  final bool phaseChanged;
  final List<MailboxError> mailboxErrors;
  final int ruleActionsApplied;
}

List<CategoryRef> _categoryRefs(Object? raw) {
  if (raw is! List<Object?>) return const <CategoryRef>[];
  return raw
      .whereType<Map<String, Object?>>()
      .map(CategoryRef.fromJson)
      .toList(growable: false);
}

List<FeedCard> _cards(Object? raw) {
  if (raw is! List<Object?>) return const <FeedCard>[];
  return raw
      .whereType<Map<String, Object?>>()
      .map(FeedCard.fromJson)
      .toList(growable: false);
}

List<MailboxError> _mailboxErrors(Object? raw) {
  if (raw is! List<Object?>) return const <MailboxError>[];
  return raw
      .whereType<Map<String, Object?>>()
      .map(MailboxError.fromJson)
      .toList(growable: false);
}
