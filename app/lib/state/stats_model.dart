import 'package:flutter/foundation.dart' show ChangeNotifier;

import '../api/api_client.dart';
import '../api/models/stats.dart';
import 'filed_model.dart' show LoadState;

/// Drives Stats (S9 7.4). In memory only.
class StatsModel extends ChangeNotifier {
  StatsModel({required ApiClient api}) : _api = api;

  final ApiClient _api;

  LoadState _state = LoadState.loading;
  Stats? _stats;

  LoadState get state => _state;
  Stats? get stats => _stats;

  Future<void> load() async {
    _state = LoadState.loading;
    notifyListeners();
    try {
      _stats = await _api.getStats();
      _state = LoadState.ready;
    } on Object {
      _state = LoadState.failed;
    }
    notifyListeners();
  }
}
