import 'package:flutter/foundation.dart' show ChangeNotifier;

import '../api/api_client.dart';
import '../api/models/history.dart';
import 'filed_model.dart' show LoadState;

/// Drives History (S9 7.2): filter, pages, load more. In memory only.
class HistoryModel extends ChangeNotifier {
  HistoryModel({required ApiClient api}) : _api = api;

  final ApiClient _api;

  LoadState _state = LoadState.loading;
  HistoryFilter _filter = HistoryFilter.all;
  List<HistoryEntry> _entries = const <HistoryEntry>[];
  String? _cursor;
  bool _loadingMore = false;
  int _generation = 0;

  LoadState get state => _state;
  HistoryFilter get filter => _filter;
  List<HistoryEntry> get entries => _entries;
  bool get hasMore => _cursor != null;

  /// Loads the first page for [filter] (the current one when null).
  Future<void> load([HistoryFilter? filter]) async {
    if (filter != null) {
      _filter = filter;
    }
    final generation = ++_generation;
    _state = LoadState.loading;
    _entries = const <HistoryEntry>[];
    _cursor = null;
    notifyListeners();
    try {
      final page = await _api.listHistory(filter: _filter);
      if (generation != _generation) {
        return;
      }
      _entries = page.entries;
      _cursor = page.nextCursor;
      _state = LoadState.ready;
    } on Object {
      if (generation != _generation) {
        return;
      }
      _state = LoadState.failed;
    }
    notifyListeners();
  }

  Future<void> loadMore() async {
    final cursor = _cursor;
    if (cursor == null || _loadingMore || _state != LoadState.ready) {
      return;
    }
    _loadingMore = true;
    final generation = _generation;
    try {
      final page = await _api.listHistory(filter: _filter, cursor: cursor);
      if (generation != _generation) {
        return;
      }
      _entries = [..._entries, ...page.entries];
      _cursor = page.nextCursor;
      notifyListeners();
    } on Object {
      // Keep what is shown; the next scroll to the end tries again.
    } finally {
      _loadingMore = false;
    }
  }
}
