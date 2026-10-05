import 'dart:convert';
import 'package:http/http.dart' as http;

import 'api_client.dart';
import 'models/auth.dart';
import 'models/feed.dart';
import 'models/session.dart';
import 'models/swipe.dart';

typedef UnauthenticatedHandler = void Function();

class HttpApiClient implements ApiClient {
  HttpApiClient({
    required http.Client client,
    required Uri origin,
    required this.onUnauthenticated,
  }) : _client = client,
       _origin = origin;

  final http.Client _client;
  final Uri _origin;
  final UnauthenticatedHandler onUnauthenticated;

  String? _csrfToken;

  String? get csrfToken => _csrfToken;

  Uri _resolvePath(String path) {
    final cleanedPath = path.startsWith('/') ? path.substring(1) : path;
    final basePath = _origin.path.endsWith('/')
        ? _origin.path
        : '${_origin.path}/';
    final fullPath = '${basePath}api/v1/$cleanedPath';
    return _origin.replace(path: fullPath);
  }

  @override
  Future<Session> getSession() async {
    final data = await send('GET', 'session');
    if (data == null) {
      return Session.anonymous();
    }
    final session = Session.fromJson(data);
    _csrfToken = session.csrfToken;
    return session;
  }

  @override
  Future<void> signOut() async {
    await send('POST', 'auth/sign-out', expect: const {204});
  }

  @override
  Future<Uri> startAuth({
    required AuthIntent intent,
    String? inviteToken,
    String? mailboxId,
  }) async {
    final data = await send(
      'POST',
      'auth/google/start',
      body: {
        'intent': intent.wire,
        'invite_token': inviteToken,
        'mailbox_id': mailboxId,
      },
    );
    final raw = data?['authorization_url'] as String?;
    if (raw == null) {
      throw const NetworkException();
    }
    return Uri.parse(raw);
  }

  @override
  Future<void> createInviteRequest() async {
    await send('POST', 'invite-requests', body: const {}, expect: const {202});
  }

  @override
  Future<FeedPage> feedNext({
    String? cursor,
    int limit = 20,
    bool refresh = false,
  }) async {
    final data = await send(
      'POST',
      'feed/next',
      body: {'cursor': cursor, 'limit': limit, 'refresh': refresh},
    );
    if (data == null) {
      throw const NetworkException();
    }
    return FeedPage.fromJson(data);
  }

  @override
  Future<SwipeResult> swipe(
    SwipeRequest req, {
    required String idempotencyKey,
  }) async {
    final data = await send(
      'POST',
      'swipes',
      body: req.toJson(),
      idempotencyKey: idempotencyKey,
    );
    if (data == null) {
      throw const NetworkException();
    }
    return SwipeResult.fromJson(data);
  }

  @override
  Future<UndoResult> undo(String undoToken) async {
    final data = await send(
      'POST',
      'swipes/undo',
      body: {'undo_token': undoToken},
    );
    if (data == null) {
      throw const NetworkException();
    }
    return UndoResult.fromJson(data);
  }

  @override
  Future<void> createBlockRule(String promptRef) async {
    await send(
      'POST',
      'rules',
      body: {'kind': 'block_person', 'prompt_ref': promptRef},
      expect: const {201},
    );
  }

  @override
  Future<void> declineBlockPrompt(String promptRef) async {
    await send(
      'POST',
      'block-prompts/decline',
      body: {'prompt_ref': promptRef},
      expect: const {204},
    );
  }

  /// Shared plumbing for every endpoint added by later tasks.
  Future<Map<String, Object?>?> send(
    String method,
    String path, {
    Map<String, Object?>? body,
    String? idempotencyKey,
    Set<int> expect = const {200},
  }) async {
    return _sendInternal(
      method,
      path,
      body: body,
      idempotencyKey: idempotencyKey,
      expect: expect,
      canRetryCsrf: true,
    );
  }

  Future<Map<String, Object?>?> _sendInternal(
    String method,
    String path, {
    Map<String, Object?>? body,
    String? idempotencyKey,
    required Set<int> expect,
    required bool canRetryCsrf,
  }) async {
    final uri = _resolvePath(path);
    final upperMethod = method.toUpperCase();

    final headers = <String, String>{'Accept': 'application/json'};

    if (body != null) {
      headers['Content-Type'] = 'application/json; charset=utf-8';
    }

    if (upperMethod == 'POST' ||
        upperMethod == 'PUT' ||
        upperMethod == 'PATCH' ||
        upperMethod == 'DELETE') {
      if (_csrfToken != null) {
        headers['X-CSRF-Token'] = _csrfToken!;
      }
      if (idempotencyKey != null) {
        headers['Idempotency-Key'] = idempotencyKey;
      }
    }

    http.Response response;
    try {
      final request = http.Request(upperMethod, uri);
      request.headers.addAll(headers);
      if (body != null) {
        request.body = jsonEncode(body);
      }
      final streamedResponse = await _client.send(request);
      response = await http.Response.fromStream(streamedResponse);
    } on http.ClientException {
      throw const NetworkException();
    } on Object catch (e) {
      if (e is! ApiException && e is! NetworkException) {
        throw const NetworkException();
      }
      rethrow;
    }

    if (expect.contains(response.statusCode)) {
      if (response.statusCode == 204 || response.body.trim().isEmpty) {
        return null;
      }
      try {
        final decoded = jsonDecode(response.body);
        if (decoded is Map<String, Object?>) {
          return decoded;
        }
        if (decoded is Map) {
          return decoded.cast<String, Object?>();
        }
        return null;
      } catch (_) {
        return null;
      }
    }

    // Status outside expect: parse problem JSON
    final exception = _parseProblem(response);

    if (response.statusCode == 401) {
      onUnauthenticated();
      throw exception;
    }

    if (response.statusCode == 403 &&
        exception.code == 'csrf_failed' &&
        canRetryCsrf) {
      // Reload session, retry once
      await getSession();
      return _sendInternal(
        method,
        path,
        body: body,
        idempotencyKey: idempotencyKey,
        expect: expect,
        canRetryCsrf: false,
      );
    }

    throw exception;
  }

  ApiException _parseProblem(http.Response response) {
    final requestIdHeader = response.headers['x-request-id'] ?? '';
    try {
      final decoded = jsonDecode(response.body);
      if (decoded is Map<String, Object?>) {
        final code = (decoded['code'] as String?) ?? 'unknown';
        final reqId = (decoded['request_id'] as String?) ?? requestIdHeader;
        final mailboxId = decoded['mailbox_id'] as String?;
        final retryAfterSeconds = decoded['retry_after_seconds'] as int?;
        final rawFields = decoded['fields'] as List<Object?>?;
        final fields = rawFields != null
            ? rawFields.whereType<String>().toList(growable: false)
            : const <String>[];

        return ApiException(
          status: response.statusCode,
          code: code,
          requestId: reqId,
          mailboxId: mailboxId,
          retryAfterSeconds: retryAfterSeconds,
          fields: fields,
        );
      }
    } catch (_) {
      // Non-problem error body
    }

    return ApiException(
      status: response.statusCode,
      code: 'unknown',
      requestId: requestIdHeader,
    );
  }
}
