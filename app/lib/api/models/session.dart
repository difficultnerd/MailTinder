enum SessionState { anonymous, preAuth, pendingInviteRequest, authenticated }

enum MailboxStatus { connected, needsSignIn, consentBlocked, unknown }

class SessionUser {
  const SessionUser({required this.userId, required this.isAdmin});

  factory SessionUser.fromJson(Map<String, Object?> json) {
    return SessionUser(
      userId: (json['user_id'] as String?) ?? '',
      isAdmin: (json['is_admin'] as bool?) ?? false,
    );
  }

  final String userId;
  final bool isAdmin;
}

class Mailbox {
  const Mailbox({
    required this.mailboxId,
    required this.provider,
    required this.emailAddress,
    required this.status,
  });

  factory Mailbox.fromJson(Map<String, Object?> json) {
    final statusStr = json['status'] as String?;
    final status = switch (statusStr) {
      'connected' => MailboxStatus.connected,
      'needs_sign_in' => MailboxStatus.needsSignIn,
      'consent_blocked' => MailboxStatus.consentBlocked,
      _ => MailboxStatus.unknown,
    };

    return Mailbox(
      mailboxId: (json['mailbox_id'] as String?) ?? '',
      provider: (json['provider'] as String?) ?? '',
      emailAddress: (json['email_address'] as String?) ?? '',
      status: status,
    );
  }

  final String mailboxId;
  final String provider;
  final String emailAddress;
  final MailboxStatus status;
}

class Session {
  const Session({
    required this.state,
    this.csrfToken,
    this.user,
    this.pendingInviteEmail,
    this.stepUpValidUntil,
    this.mailboxes = const [],
  });

  factory Session.anonymous() {
    return const Session(state: SessionState.anonymous);
  }

  factory Session.fromJson(Map<String, Object?> json) {
    final stateStr = json['state'] as String?;
    final state = switch (stateStr) {
      'anonymous' => SessionState.anonymous,
      'pre_auth' => SessionState.preAuth,
      'pending_invite_request' => SessionState.pendingInviteRequest,
      'authenticated' => SessionState.authenticated,
      _ => SessionState.anonymous,
    };

    final csrfToken = json['csrf_token'] as String?;

    final userJson = json['user'] as Map<String, Object?>?;
    final user = userJson != null ? SessionUser.fromJson(userJson) : null;

    final pendingInviteEmail = json['pending_invite_email'] as String?;

    final stepUpStr = json['step_up_valid_until'] as String?;
    final stepUpValidUntil = stepUpStr != null
        ? DateTime.parse(stepUpStr).toUtc()
        : null;

    final rawMailboxes = json['mailboxes'] as List<Object?>?;
    final mailboxes = rawMailboxes != null
        ? rawMailboxes
              .whereType<Map<String, Object?>>()
              .map(Mailbox.fromJson)
              .toList(growable: false)
        : const <Mailbox>[];

    return Session(
      state: state,
      csrfToken: csrfToken,
      user: user,
      pendingInviteEmail: pendingInviteEmail,
      stepUpValidUntil: stepUpValidUntil,
      mailboxes: mailboxes,
    );
  }

  final SessionState state;
  final String? csrfToken;
  final SessionUser? user;
  final String? pendingInviteEmail;
  final DateTime? stepUpValidUntil;
  final List<Mailbox> mailboxes;
}
