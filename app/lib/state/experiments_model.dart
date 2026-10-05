import 'package:flutter/foundation.dart' show ChangeNotifier;

import '../api/api_client.dart';
import '../api/models/experiments.dart';
import '../consent_text.dart';
import 'filed_model.dart' show LoadState;

enum ExperimentsNotice { none, consentChanged, failed }

/// Drives Experiments (S9 7.8, CL-02). In memory only.
class ExperimentsModel extends ChangeNotifier {
  ExperimentsModel({required ApiClient api}) : _api = api;

  final ApiClient _api;

  LoadState _state = LoadState.loading;
  MyExperiments? _mine;
  ExperimentsNotice _notice = ExperimentsNotice.none;
  bool _busy = false;

  LoadState get state => _state;
  MyExperiments? get mine => _mine;
  ExperimentsNotice get notice => _notice;
  bool get busy => _busy;

  /// On only when opted in to the current consent version.
  bool get isOn {
    final m = _mine;
    return m != null &&
        m.optedIn &&
        m.consentVersion == m.currentConsentVersion;
  }

  bool get paused => _mine != null && !_mine!.available;

  Future<void> load() async {
    _state = LoadState.loading;
    notifyListeners();
    try {
      _mine = await _api.getMyExperiments();
      _state = LoadState.ready;
    } on Object {
      _state = LoadState.failed;
    }
    notifyListeners();
  }

  Future<void> turnOn() => _put(optedIn: true);

  Future<void> turnOff() => _put(optedIn: false);

  Future<void> _put({required bool optedIn}) async {
    final m = _mine;
    if (m == null || _busy) {
      return;
    }
    _notice = ExperimentsNotice.none;
    // The app's text is stale: never send an opt-in for a version the user
    // was not shown.
    if (optedIn && m.currentConsentVersion != kExperimentsConsentVersion) {
      _notice = ExperimentsNotice.consentChanged;
      notifyListeners();
      return;
    }
    _busy = true;
    notifyListeners();
    try {
      _mine = await _api.putMyExperiments(
        optedIn: optedIn,
        consentVersion: kExperimentsConsentVersion,
      );
    } on ApiException catch (e) {
      if (e.code == 'consent_outdated') {
        _notice = ExperimentsNotice.consentChanged;
        await _reload();
      } else if (e.code == 'experiment_unavailable') {
        _mine = _asPaused(m);
      } else {
        _notice = ExperimentsNotice.failed;
      }
    } on Object {
      _notice = ExperimentsNotice.failed;
    }
    _busy = false;
    notifyListeners();
  }

  Future<void> _reload() async {
    try {
      _mine = await _api.getMyExperiments();
    } on Object {
      // Keep what we had.
    }
  }

  MyExperiments _asPaused(MyExperiments m) => MyExperiments(
    available: false,
    optedIn: m.optedIn,
    consentVersion: m.consentVersion,
    currentConsentVersion: m.currentConsentVersion,
    optedInAt: m.optedInAt,
  );
}
