/// S7 API-ADM-1 to API-ADM-16 shapes.
library;

enum InviteStatus {
  pending,
  used,
  revoked,
  expired;

  static InviteStatus parse(String? s) => switch (s) {
    'used' => used,
    'revoked' => revoked,
    'expired' => expired,
    _ => pending,
  };
}

DateTime _date(Object? v) => v is String
    ? DateTime.parse(v).toUtc()
    : DateTime.fromMillisecondsSinceEpoch(0, isUtc: true);

class Invite {
  const Invite({
    required this.inviteId,
    required this.emailAddress,
    required this.status,
    required this.createdAt,
    required this.expiresAt,
    required this.lastSentAt,
  });

  factory Invite.fromJson(Map<String, Object?> json) => Invite(
    inviteId: (json['invite_id'] as String?) ?? '',
    emailAddress: (json['email_address'] as String?) ?? '',
    status: InviteStatus.parse(json['status'] as String?),
    createdAt: _date(json['created_at']),
    expiresAt: _date(json['expires_at']),
    lastSentAt: _date(json['last_sent_at']),
  );

  final String inviteId;
  final String emailAddress;
  final InviteStatus status;
  final DateTime createdAt;
  final DateTime expiresAt;
  final DateTime lastSentAt;
}

class InviteRequest {
  const InviteRequest({
    required this.requestId,
    required this.emailAddress,
    required this.createdAt,
  });

  factory InviteRequest.fromJson(Map<String, Object?> json) => InviteRequest(
    requestId: (json['request_id'] as String?) ?? '',
    emailAddress: (json['email_address'] as String?) ?? '',
    createdAt: _date(json['created_at']),
  );

  final String requestId;
  final String emailAddress;
  final DateTime createdAt;
}

class AdminUser {
  const AdminUser({
    required this.userId,
    required this.emailAddress,
    required this.createdAt,
    required this.isAdmin,
    required this.signedIn,
    required this.mailboxCount,
    this.lastSeenAt,
  });

  factory AdminUser.fromJson(Map<String, Object?> json) {
    final seen = json['last_seen_at'] as String?;
    return AdminUser(
      userId: (json['user_id'] as String?) ?? '',
      emailAddress: (json['email_address'] as String?) ?? '',
      createdAt: _date(json['created_at']),
      isAdmin: (json['is_admin'] as bool?) ?? false,
      signedIn: (json['signed_in'] as bool?) ?? false,
      mailboxCount: (json['mailbox_count'] as num?)?.toInt() ?? 0,
      lastSeenAt: seen == null ? null : DateTime.parse(seen).toUtc(),
    );
  }

  final String userId;
  final String emailAddress;
  final DateTime createdAt;
  final bool isAdmin;
  final bool signedIn;
  final int mailboxCount;
  final DateTime? lastSeenAt;
}

class Paged<T> {
  const Paged({required this.items, this.nextCursor});

  final List<T> items;
  final String? nextCursor;

  static Paged<T> parse<T>(
    Map<String, Object?>? json,
    String key,
    T Function(Map<String, Object?>) fromJson,
  ) {
    final raw = json?[key];
    return Paged<T>(
      items: raw is List<Object?>
          ? raw.whereType<Map<String, Object?>>().map(fromJson).toList()
          : <T>[],
      nextCursor: json?['next_cursor'] as String?,
    );
  }
}
