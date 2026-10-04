import 'models/session.dart';

abstract class ApiClient {
  /// GET /api/v1/session. Never 401 (S7 5.2); creates a pre_auth session when none exists.
  Future<Session> getSession();

  /// POST /api/v1/auth/sign-out -> 204.
  Future<void> signOut();
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
