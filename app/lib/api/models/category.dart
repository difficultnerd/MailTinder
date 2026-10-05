/// S7 models for the Filed tab: `Category`, `CategoryMailboxCount`,
/// `FiledMessage` and the `GET /categories/{id}/messages` page (API-CAT-1 to
/// API-CAT-5).
library;

import 'feed.dart';

/// One mailbox's share of a category's message count (S7 `Category.per_mailbox`).
class CategoryMailboxCount {
  const CategoryMailboxCount({
    required this.mailboxId,
    required this.messageCount,
  });

  factory CategoryMailboxCount.fromJson(Map<String, Object?> json) {
    return CategoryMailboxCount(
      mailboxId: (json['mailbox_id'] as String?) ?? '',
      messageCount: (json['message_count'] as num?)?.toInt() ?? 0,
    );
  }

  final String mailboxId;
  final int messageCount;
}

/// A filing category across all mailboxes (S7 `Category`).
class Category {
  const Category({
    required this.categoryId,
    required this.name,
    required this.messageCount,
    required this.perMailbox,
  });

  factory Category.fromJson(Map<String, Object?> json) {
    final rawPerMailbox = json['per_mailbox'];
    final perMailbox = rawPerMailbox is List<Object?>
        ? rawPerMailbox
              .whereType<Map<String, Object?>>()
              .map(CategoryMailboxCount.fromJson)
              .toList(growable: false)
        : const <CategoryMailboxCount>[];
    return Category(
      categoryId: (json['category_id'] as String?) ?? '',
      name: (json['name'] as String?) ?? '',
      messageCount: (json['message_count'] as num?)?.toInt() ?? 0,
      perMailbox: perMailbox,
    );
  }

  final String categoryId;
  final String name;
  final int messageCount;
  final List<CategoryMailboxCount> perMailbox;
}

/// One filed message (S7 `FiledMessage`).
class FiledMessage {
  const FiledMessage({
    required this.mailboxId,
    required this.messageId,
    required this.senderName,
    required this.subject,
    required this.receivedAt,
    required this.providerWebUrl,
  });

  factory FiledMessage.fromJson(Map<String, Object?> json) {
    return FiledMessage(
      mailboxId: (json['mailbox_id'] as String?) ?? '',
      messageId: (json['message_id'] as String?) ?? '',
      senderName: (json['sender_name'] as String?) ?? '',
      subject: (json['subject'] as String?) ?? '',
      receivedAt: DateTime.parse(
        (json['received_at'] as String?) ?? '',
      ).toUtc(),
      providerWebUrl: Uri.parse((json['provider_web_url'] as String?) ?? ''),
    );
  }

  final String mailboxId;
  final String messageId;
  final String senderName;
  final String subject;
  final DateTime receivedAt;
  final Uri providerWebUrl;
}

/// One page of filed messages with the per-mailbox errors alongside
/// (S7 API-CAT-5).
class FiledMessagePage {
  const FiledMessagePage({
    required this.messages,
    required this.nextCursor,
    required this.mailboxErrors,
  });

  factory FiledMessagePage.fromJson(Map<String, Object?> json) {
    final rawMessages = json['messages'];
    final messages = rawMessages is List<Object?>
        ? rawMessages
              .whereType<Map<String, Object?>>()
              .map(FiledMessage.fromJson)
              .toList(growable: false)
        : const <FiledMessage>[];

    final rawErrors = json['mailbox_errors'];
    final mailboxErrors = rawErrors is List<Object?>
        ? rawErrors
              .whereType<Map<String, Object?>>()
              .map(MailboxError.fromJson)
              .toList(growable: false)
        : const <MailboxError>[];

    return FiledMessagePage(
      messages: messages,
      nextCursor: json['next_cursor'] as String?,
      mailboxErrors: mailboxErrors,
    );
  }

  final List<FiledMessage> messages;
  final String? nextCursor;
  final List<MailboxError> mailboxErrors;
}
