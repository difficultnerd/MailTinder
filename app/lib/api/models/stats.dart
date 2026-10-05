/// S7 API-STAT-1 response.
library;

import 'swipe.dart';

class Stats {
  const Stats({
    required this.emailsTriaged,
    required this.sendersUnsubscribed,
    required this.unsubscribesConfirmed,
    required this.mailStoppedPerYear,
    required this.achievements,
  });

  factory Stats.fromJson(Map<String, Object?> json) {
    final raw = json['achievements'];
    return Stats(
      emailsTriaged: (json['emails_triaged'] as num?)?.toInt() ?? 0,
      sendersUnsubscribed: (json['senders_unsubscribed'] as num?)?.toInt() ?? 0,
      unsubscribesConfirmed:
          (json['unsubscribes_confirmed'] as num?)?.toInt() ?? 0,
      mailStoppedPerYear: (json['mail_stopped_per_year'] as num?)?.toInt() ?? 0,
      achievements: raw is List<Object?>
          ? raw
                .whereType<Map<String, Object?>>()
                .map(Achievement.fromJson)
                .toList(growable: false)
          : const <Achievement>[],
    );
  }

  final int emailsTriaged;
  final int sendersUnsubscribed;
  final int unsubscribesConfirmed;
  final int mailStoppedPerYear;
  final List<Achievement> achievements;
}
