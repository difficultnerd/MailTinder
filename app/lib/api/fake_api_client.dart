import 'api_client.dart';
import 'models/auth.dart';
import 'models/session.dart';

class FakeCall {
  FakeCall(this.method, this.path, [this.body]);

  final String method;
  final String path;
  final Object? body;
}

class FakeApiClient implements ApiClient {
  final List<FakeCall> calls = [];
  Session session = Session.anonymous();
  Object? nextError; // thrown (and cleared) by the next call when set
  Uri authUrl = Uri.parse('https://accounts.google.com/o/oauth2/auth');
  bool inviteRequestSucceeds = true;

  @override
  Future<Session> getSession() async {
    calls.add(FakeCall('GET', '/api/v1/session'));
    if (nextError != null) {
      final err = nextError;
      nextError = null;
      throw err!;
    }
    return session;
  }

  @override
  Future<void> signOut() async {
    calls.add(FakeCall('POST', '/api/v1/auth/sign-out'));
    if (nextError != null) {
      final err = nextError;
      nextError = null;
      throw err!;
    }
  }

  @override
  Future<Uri> startAuth({
    required AuthIntent intent,
    String? inviteToken,
    String? mailboxId,
  }) async {
    calls.add(
      FakeCall('POST', '/api/v1/auth/google/start', {
        'intent': intent.wire,
        'invite_token': inviteToken,
        'mailbox_id': mailboxId,
      }),
    );
    if (nextError != null) {
      final err = nextError;
      nextError = null;
      throw err!;
    }
    return authUrl;
  }

  @override
  Future<void> createInviteRequest() async {
    calls.add(FakeCall('POST', '/api/v1/invite-requests', const {}));
    if (nextError != null) {
      final err = nextError;
      nextError = null;
      throw err!;
    }
    if (!inviteRequestSucceeds) {
      throw const NetworkException();
    }
  }
}
