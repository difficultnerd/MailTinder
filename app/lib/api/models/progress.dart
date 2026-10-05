/// S7 Progress model for API-PROG-1 (`GET /progress`).
library;

import 'feed.dart';

/// The current level: the year being cleared and the mail left in it.
class Level {
  const Level({required this.year, required this.remaining});

  factory Level.fromJson(Map<String, Object?> json) {
    return Level(
      year: (json['year'] as num?)?.toInt() ?? 0,
      remaining: (json['remaining'] as num?)?.toInt() ?? 0,
    );
  }

  final int year;
  final int remaining;
}

/// The inbox meter and level (GM-01, GM-04).
class Progress {
  const Progress({
    required this.inboxCount,
    required this.mailboxErrors,
    required this.level,
  });

  factory Progress.fromJson(Map<String, Object?> json) {
    final errors = json['mailbox_errors'];
    final level = json['level'];
    return Progress(
      inboxCount: (json['inbox_count'] as num?)?.toInt() ?? 0,
      mailboxErrors: errors is List<Object?>
          ? errors
                .whereType<Map<String, Object?>>()
                .map(MailboxError.fromJson)
                .toList(growable: false)
          : const <MailboxError>[],
      level: level is Map<String, Object?> ? Level.fromJson(level) : null,
    );
  }

  final int inboxCount;
  final List<MailboxError> mailboxErrors;
  final Level? level;
}
