import 'models/auth.dart';
import 'models/category.dart';
import 'models/feed.dart';
import 'models/needs_attention.dart';
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

  /// POST /api/v1/rules {kind: "file", mailbox_id, message_id, category_id}
  /// (API-RULE-2, FL-04 AC2).
  Future<void> createFileRule({
    required String mailboxId,
    required String messageId,
    required String categoryId,
  });

  /// POST /api/v1/block-prompts/decline (API-RULE-5).
  Future<void> declineBlockPrompt(String promptRef);

  /// GET /api/v1/categories (API-CAT-1).
  Future<List<Category>> listCategories();

  /// PATCH /api/v1/categories/{category_id} {name} (API-CAT-3).
  Future<Category> renameCategory(String categoryId, String name);

  /// DELETE /api/v1/categories/{category_id} (API-CAT-4). Removes the label
  /// only; messages are never deleted (S3 INV-5).
  Future<void> deleteCategory(String categoryId);

  /// GET /api/v1/categories/{category_id}/messages (API-CAT-5).
  Future<FiledMessagePage> listCategoryMessages(
    String categoryId, {
    String? cursor,
    int limit = 20,
  });

  /// GET /api/v1/needs-attention (API-NA-1).
  Future<NeedsAttentionPage> listNeedsAttention({
    String? cursor,
    int limit = 20,
  });

  /// POST /api/v1/needs-attention/{item_id}/resolve (API-NA-2).
  Future<void> resolveNeedsAttention(String itemId);

  /// POST /api/v1/needs-attention/{item_id}/dismiss (API-NA-3).
  Future<void> dismissNeedsAttention(String itemId);
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
