import 'package:flutter/foundation.dart' show ChangeNotifier;

import '../api/api_client.dart';
import '../api/models/admin.dart' show Paged;
import '../api/models/bakeoff.dart';

enum BakeoffState { loading, ready, notEnoughData, versionsMixed, failed }

/// Drives the Bake-off report (S9 7.7, API-ADM-8 to API-ADM-14). In memory
/// only; the writes run behind Confirm it's you in the screen.
class BakeoffModel extends ChangeNotifier {
  BakeoffModel({required ApiClient api, DateTime Function()? now})
    : _api = api {
    final today = (now ?? DateTime.now)();
    final to = DateTime.utc(today.year, today.month, today.day);
    _query = BakeoffQuery(
      from: to.subtract(const Duration(days: 30)), // [DEFAULT]
      to: to,
    );
  }

  final ApiClient _api;
  late BakeoffQuery _query;
  BakeoffState _state = BakeoffState.loading;
  BakeoffReport? _report;
  Map<String, List<String>>? _versionsPresent;
  ClassifierExperiment? _experiment;
  List<SnapshotSummary> _snapshots = const [];
  String? _lastSavedName;
  int _generation = 0;

  BakeoffState get state => _state;
  BakeoffReport? get report => _report;
  Map<String, List<String>>? get versionsPresent => _versionsPresent;
  BakeoffQuery get query => _query;
  ClassifierExperiment? get experiment => _experiment;
  List<SnapshotSummary> get snapshots => _snapshots;
  String? get lastSavedName => _lastSavedName;

  /// Setting the query reloads the report.
  set query(BakeoffQuery q) {
    _query = q;
    loadReport();
  }

  /// Loads the report, the kill switches and the snapshot list in parallel.
  Future<void> load() async {
    await Future.wait([loadReport(), loadSwitches(), refreshSnapshots()]);
  }

  Future<void> loadReport() async {
    final generation = ++_generation;
    _state = BakeoffState.loading;
    notifyListeners();
    try {
      final report = await _api.getBakeoff(_query);
      if (generation != _generation) {
        return;
      }
      _report = report;
      _versionsPresent = report.versionsPresent;
      final n = report.labelledSwipes;
      _state = n == null || n < report.minCellSize
          ? BakeoffState.notEnoughData
          : BakeoffState.ready;
    } on ApiException catch (e) {
      if (generation != _generation) {
        return;
      }
      if (e.status == 409 && e.code == 'versions_mixed') {
        enterVersionsMixed(e);
      } else {
        _state = BakeoffState.failed;
      }
    } on Object {
      if (generation != _generation) {
        return;
      }
      _state = BakeoffState.failed;
    }
    notifyListeners();
  }

  /// Moves to the versions-mixed state from a `409 versions_mixed` problem
  /// (CL-04 AC5), whether it came from the report or from a save.
  void enterVersionsMixed(ApiException e) {
    _generation++;
    _report = null;
    _versionsPresent = parseVersionsPresent(e.problem?['versions_present']);
    _state = BakeoffState.versionsMixed;
    notifyListeners();
  }

  Future<void> loadSwitches() async {
    try {
      _experiment = await _api.getClassifierExperiment();
    } on Object {
      _experiment = null;
    }
    notifyListeners();
  }

  void applySwitches(ClassifierExperiment e) {
    _experiment = e;
    notifyListeners();
  }

  Future<void> refreshSnapshots() async {
    try {
      final Paged<SnapshotSummary> page = await _api.listSnapshots();
      _snapshots = page.items;
    } on Object {
      _snapshots = const [];
    }
    notifyListeners();
  }

  void markSaved(String name) {
    _lastSavedName = name;
    notifyListeners();
  }

  void clearSaved() {
    _lastSavedName = null;
    notifyListeners();
  }
}
