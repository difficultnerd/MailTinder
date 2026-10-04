import 'dart:convert';
import 'package:app/api/api_client.dart';
import 'package:app/api/http_api_client.dart';
import 'package:app/api/models/session.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';

void main() {
  final testOrigin = Uri.parse('https://mailtinder.app');

  test('ASVS V14.3.1 HttpApiClient calls onUnauthenticated on 401', () async {
    var unauthenticatedCalled = false;
    final client = MockClient((request) async {
      return http.Response(
        jsonEncode({
          'type': 'https://mailtinder.app/problems/unauthenticated',
          'title': 'Unauthenticated',
          'status': 401,
          'code': 'unauthenticated',
          'request_id': 'req-401',
        }),
        401,
        headers: {
          'content-type': 'application/problem+json',
          'x-request-id': 'req-401',
        },
      );
    });

    final apiClient = HttpApiClient(
      client: client,
      origin: testOrigin,
      onUnauthenticated: () {
        unauthenticatedCalled = true;
      },
    );

    await expectLater(
      () => apiClient.send('POST', 'swipes'),
      throwsA(isA<ApiException>().having((e) => e.status, 'status', 401)),
    );
    expect(unauthenticatedCalled, isTrue);
  });

  test('csrf header sent on post and not on get', () async {
    final sentHeaders = <String, Map<String, String>>{};

    final client = MockClient((request) async {
      sentHeaders[request.method] = request.headers;
      if (request.url.path.endsWith('/api/v1/session')) {
        return http.Response(
          jsonEncode({
            'state': 'authenticated',
            'csrf_token': 'test-csrf-token-123',
          }),
          200,
          headers: {'content-type': 'application/json'},
        );
      }
      return http.Response('', 204);
    });

    final apiClient = HttpApiClient(
      client: client,
      origin: testOrigin,
      onUnauthenticated: () {},
    );

    // Initial getSession to obtain CSRF token
    await apiClient.getSession();
    expect(sentHeaders['GET']?.containsKey('X-CSRF-Token'), isFalse);

    // POST request should include X-CSRF-Token
    await apiClient.send('POST', 'auth/sign-out', expect: const {204});
    expect(sentHeaders['POST']?['X-CSRF-Token'], equals('test-csrf-token-123'));
  });

  test('idempotency key header sent when given', () async {
    String? capturedKey;
    final client = MockClient((request) async {
      capturedKey = request.headers['Idempotency-Key'];
      return http.Response('{}', 200);
    });

    final apiClient = HttpApiClient(
      client: client,
      origin: testOrigin,
      onUnauthenticated: () {},
    );

    await apiClient.send('POST', 'swipes', idempotencyKey: 'idemp-xyz-987');

    expect(capturedKey, equals('idemp-xyz-987'));
  });

  test('problem json parsed into ApiException', () async {
    final client = MockClient((request) async {
      return http.Response(
        jsonEncode({
          'type': 'https://mailtinder.app/problems/rate_limited',
          'title': 'Too many requests',
          'status': 429,
          'code': 'rate_limited',
          'request_id': '6f1c2a1e-1d7e-4b8e-9f62-0d4a3c2b1a90',
          'mailbox_id': 'mbx-uuid-1',
          'retry_after_seconds': 30,
          'fields': ['body.limit'],
        }),
        429,
        headers: {
          'content-type': 'application/problem+json',
          'x-request-id': '6f1c2a1e-1d7e-4b8e-9f62-0d4a3c2b1a90',
        },
      );
    });

    final apiClient = HttpApiClient(
      client: client,
      origin: testOrigin,
      onUnauthenticated: () {},
    );

    try {
      await apiClient.send('POST', 'feed/next');
      fail('Expected ApiException');
    } on ApiException catch (e) {
      expect(e.status, equals(429));
      expect(e.code, equals('rate_limited'));
      expect(e.requestId, equals('6f1c2a1e-1d7e-4b8e-9f62-0d4a3c2b1a90'));
      expect(e.mailboxId, equals('mbx-uuid-1'));
      expect(e.retryAfterSeconds, equals(30));
      expect(e.fields, equals(['body.limit']));
    }
  });

  test('non problem error body gives code unknown', () async {
    final client = MockClient((request) async {
      return http.Response(
        'Internal Server Error',
        500,
        headers: {'x-request-id': 'trace-err-500'},
      );
    });

    final apiClient = HttpApiClient(
      client: client,
      origin: testOrigin,
      onUnauthenticated: () {},
    );

    try {
      await apiClient.send('GET', 'session');
      fail('Expected ApiException');
    } on ApiException catch (e) {
      expect(e.status, equals(500));
      expect(e.code, equals('unknown'));
      expect(e.requestId, equals('trace-err-500'));
    }
  });

  test('csrf_failed refreshes session and retries once', () async {
    var sessionCallCount = 0;
    var postCallCount = 0;
    final tokenHistory = <String?>[];

    final client = MockClient((request) async {
      if (request.url.path.endsWith('/api/v1/session')) {
        sessionCallCount++;
        return http.Response(
          jsonEncode({
            'state': 'authenticated',
            'csrf_token': 'new-csrf-token-$sessionCallCount',
          }),
          200,
        );
      }

      if (request.url.path.endsWith('/api/v1/swipes')) {
        postCallCount++;
        tokenHistory.add(request.headers['X-CSRF-Token']);
        if (postCallCount == 1) {
          return http.Response(
            jsonEncode({
              'code': 'csrf_failed',
              'status': 403,
              'request_id': 'csrf-err-1',
            }),
            403,
          );
        }
        return http.Response('{"status": "ok"}', 200);
      }

      return http.Response('Not Found', 404);
    });

    final apiClient = HttpApiClient(
      client: client,
      origin: testOrigin,
      onUnauthenticated: () {},
    );

    // Initial session
    await apiClient.getSession();
    expect(sessionCallCount, equals(1));

    // Send post - first fails with csrf_failed, retries with refreshed token
    final result = await apiClient.send('POST', 'swipes');
    expect(result, equals({'status': 'ok'}));
    expect(sessionCallCount, equals(2));
    expect(postCallCount, equals(2));
    expect(tokenHistory[0], equals('new-csrf-token-1'));
    expect(tokenHistory[1], equals('new-csrf-token-2'));
  });

  test('client exception becomes NetworkException', () async {
    final client = MockClient((request) async {
      throw http.ClientException('Failed to connect to host');
    });

    final apiClient = HttpApiClient(
      client: client,
      origin: testOrigin,
      onUnauthenticated: () {},
    );

    expect(
      () => apiClient.send('GET', 'session'),
      throwsA(isA<NetworkException>()),
    );
  });

  test('session json parses every state and mailbox status', () {
    final json = {
      'state': 'authenticated',
      'csrf_token': 'csrf-xyz',
      'step_up_valid_until': '2026-10-03T14:24:18Z',
      'user': {'user_id': 'u-123', 'is_admin': true},
      'pending_invite_email': 'test@example.com',
      'mailboxes': [
        {
          'mailbox_id': 'm-1',
          'provider': 'gmail',
          'email_address': 'm1@example.com',
          'status': 'connected',
        },
        {
          'mailbox_id': 'm-2',
          'provider': 'graph',
          'email_address': 'm2@example.com',
          'status': 'needs_sign_in',
        },
        {
          'mailbox_id': 'm-3',
          'provider': 'graph',
          'email_address': 'm3@example.com',
          'status': 'consent_blocked',
        },
        {
          'mailbox_id': 'm-4',
          'provider': 'other',
          'email_address': 'm4@example.com',
          'status': 'something_else',
        },
      ],
      'unknown_new_field': 'future_data',
    };

    final session = Session.fromJson(json);
    expect(session.state, equals(SessionState.authenticated));
    expect(session.csrfToken, equals('csrf-xyz'));
    expect(
      session.stepUpValidUntil,
      equals(DateTime.utc(2026, 10, 3, 14, 24, 18)),
    );
    expect(session.user?.userId, equals('u-123'));
    expect(session.user?.isAdmin, isTrue);
    expect(session.pendingInviteEmail, equals('test@example.com'));

    expect(session.mailboxes.length, equals(4));
    expect(session.mailboxes[0].status, equals(MailboxStatus.connected));
    expect(session.mailboxes[1].status, equals(MailboxStatus.needsSignIn));
    expect(session.mailboxes[2].status, equals(MailboxStatus.consentBlocked));
    expect(session.mailboxes[3].status, equals(MailboxStatus.unknown));

    // States
    final s1 = Session.fromJson({'state': 'anonymous'});
    expect(s1.state, equals(SessionState.anonymous));

    final s2 = Session.fromJson({'state': 'pre_auth'});
    expect(s2.state, equals(SessionState.preAuth));

    final s3 = Session.fromJson({'state': 'pending_invite_request'});
    expect(s3.state, equals(SessionState.pendingInviteRequest));

    final s4 = Session.fromJson({'state': 'unknown_state'});
    expect(s4.state, equals(SessionState.anonymous));
  });
}
