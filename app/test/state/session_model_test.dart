import 'package:app/api/api_client.dart';
import 'package:app/api/fake_api_client.dart';
import 'package:app/api/models/session.dart';
import 'package:app/state/session_model.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  test('session model initializes with null session and loading false', () {
    final fakeApi = FakeApiClient();
    final model = SessionModel(api: fakeApi);

    expect(model.session, isNull);
    expect(model.loading, isFalse);
    expect(model.lastError, isNull);
  });

  test('refresh sets session and notifies listeners', () async {
    final fakeApi = FakeApiClient();
    fakeApi.session = const Session(state: SessionState.authenticated);
    final model = SessionModel(api: fakeApi);

    var notifyCount = 0;
    model.addListener(() {
      notifyCount++;
    });

    await model.refresh();

    expect(model.session?.state, equals(SessionState.authenticated));
    expect(model.loading, isFalse);
    expect(model.lastError, isNull);
    expect(notifyCount, greaterThan(0));
  });

  test(
    'refresh with NetworkException sets session null and sets lastError',
    () async {
      final fakeApi = FakeApiClient();
      fakeApi.nextError = const NetworkException();
      final model = SessionModel(api: fakeApi);

      await model.refresh();

      expect(model.session, isNull);
      expect(model.loading, isFalse);
      expect(model.lastError, isA<NetworkException>());
    },
  );

  test('wipe clears session and notifies registered wipe listeners', () {
    final fakeApi = FakeApiClient();
    fakeApi.session = const Session(state: SessionState.authenticated);
    final model = SessionModel(api: fakeApi);

    var wipeListenerCalled = false;
    model.addWipeListener(() {
      wipeListenerCalled = true;
    });

    model.wipe();

    expect(model.session, isNull);
    expect(model.lastError, isNull);
    expect(wipeListenerCalled, isTrue);
  });

  test(
    'signOut calls api.signOut and wipes session even if api throws',
    () async {
      final fakeApi = FakeApiClient();
      fakeApi.nextError = Exception('signOut network error');
      final model = SessionModel(api: fakeApi);

      var wiped = false;
      model.addWipeListener(() {
        wiped = true;
      });

      await model.signOut();

      expect(
        fakeApi.calls.any((call) => call.path.contains('sign-out')),
        isTrue,
      );
      expect(model.session, isNull);
      expect(wiped, isTrue);
    },
  );
}
