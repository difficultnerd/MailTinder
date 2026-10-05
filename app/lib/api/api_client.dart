import 'models/auth.dart';
import 'models/feed.dart';
import 'models/session.dart';
import 'models/swipe.dart';

abstract class ApiClient {
  /// GET /api/v1/session. Never 401 (S7 5.2); creates a pre_auth session when none exists.
  Future<Session> getSession();

  /// POST /api/v1/auth/sign-out -> 204.
  Future<void> signOut();

  /// POST /api/v1/auth/google/start. Returns the authorization_url.
  Future<Uri> startAuth({
    required AuthIntent intent,
    String? inviteToken,
    String? mailboxId,
  });

  /// POST /api/v1/invite-requests with body {} (expects 202).
  Future<void> createInviteRequest();

  /// POST /api/v1/feed/next {cursor, limit, refresh}.
  Future<FeedPage> feedNext({
    String? cursor,
    int limit = 20,
    bool refresh = false,
  });

  /// POST /api/v1/swipes (API-SW-1). [idempotencyKey] is reused on retry.
  Future<SwipeResult> swipe(SwipeRequest req, {required String idempotencyKey});

  /// POST /api/v1/swipes/undo (API-SW-2).
  Future<UndoResult> undo(String undoToken);

  /// POST /api/v1/rules {kind: block_person, prompt_ref} (API-RULE-2).
  Future<void> createBlockRule(String promptRef);

  /// POST /api/v1/block-prompts/decline (API-RULE-5).
  Future<void> declineBlockPrompt(String promptRef);

  /// GET /api/v1/mailboxes (API-MBX-1).
  Future<List<Mailbox>> listMailboxes();

  /// DELETE /api/v1/mailboxes/{id} -> 204 (API-MBX-2). Needs step-up.
  Future<void> disconnectMailbox(String mailboxId);

  /// DELETE /api/v1/account -> 202 (API-ACCT-1). Needs step-up.
  Future<DeleteAccountResult> deleteAccount();
}

class ApiException implements Exception {
  ApiException({
    required this.status,
    required this.code,
    required this.requestId,
    this.mailboxId,
    this.retryAfterSeconds,
    this.fields = const [],
  });

  final int status;
  final String
  code; // S7 4 `code`; 'unknown' when the body is not a problem document
  final String requestId; // '' when absent
  final String? mailboxId;
  final int? retryAfterSeconds;
  final List<String> fields;

  @override
  String toString() =>
      'ApiException(status: $status, code: $code, requestId: $requestId)';
}

class NetworkException implements Exception {
  const NetworkException();

  @override
  String toString() => 'NetworkException()';
}

/// DELETE /account 202 body (API-ACCT-1).
class DeleteAccountResult {
  const DeleteAccountResult({
    required this.deletionDueBy,
    required this.appFoldersNotDeleted,
  });

  factory DeleteAccountResult.fromJson(Map<String, Object?> json) {
    final due = json['deletion_due_by'] as String?;
    final raw = json['app_folders_not_deleted'] as List<Object?>?;
    return DeleteAccountResult(
      deletionDueBy: due != null
          ? DateTime.parse(due).toUtc()
          : DateTime.fromMillisecondsSinceEpoch(0, isUtc: true),
      appFoldersNotDeleted: raw == null
          ? const []
          : raw
                .whereType<Map<String, Object?>>()
                .map(
                  (m) => (
                    mailboxId: (m['mailbox_id'] as String?) ?? '',
                    emailAddress: (m['email_address'] as String?) ?? '',
                  ),
                )
                .toList(growable: false),
    );
  }

  final DateTime deletionDueBy;
  final List<({String mailboxId, String emailAddress})> appFoldersNotDeleted;
}
