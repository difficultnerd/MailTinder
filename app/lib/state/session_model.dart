import 'package:flutter/foundation.dart';

import '../api/api_client.dart';
import '../api/models/session.dart';

class SessionModel extends ChangeNotifier {
  SessionModel({required ApiClient api}) : _api = api;

  final ApiClient _api;
  final List<VoidCallback> _onWipe = [];

  Session? _session;
  bool _loading = false;
  Object? _lastError;

  Session? get session => _session;
  bool get loading => _loading;
  Object? get lastError => _lastError;

  void addWipeListener(VoidCallback f) {
    _onWipe.add(f);
  }

  void removeWipeListener(VoidCallback f) {
    _onWipe.remove(f);
  }

  Future<void> refresh() async {
    _loading = true;
    _lastError = null;
    notifyListeners();

    try {
      final s = await _api.getSession();
      _session = s;
      _lastError = null;
    } on NetworkException catch (e) {
      _session = null;
      _lastError = e;
    } on Object catch (e) {
      _lastError = e;
    } finally {
      _loading = false;
      notifyListeners();
    }
  }

  void wipe() {
    _session = null;
    _lastError = null;
    _loading = false;
    for (final callback in List<VoidCallback>.from(_onWipe)) {
      try {
        callback();
      } catch (_) {
        // ignore listener errors during wipe
      }
    }
    notifyListeners();
  }

  Future<void> signOut() async {
    try {
      await _api.signOut();
    } catch (_) {
      // errors ignored per task spec
    } finally {
      wipe();
    }
  }
}
