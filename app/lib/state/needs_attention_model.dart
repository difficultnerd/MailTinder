import 'package:flutter/foundation.dart' show ChangeNotifier;

import '../api/api_client.dart';
import '../api/models/needs_attention.dart';
import '../copy.dart';

/// Loading states for the Needs Attention tab (S9 section 6).
enum NaLoadState { loading, ready, failed }

/// Drives the Needs Attention tab: the open item list, the badge count, and
/// Done and Dismiss (S7 API-NA-1 to API-NA-3, S9 section 6).
///
/// The list lives in memory only; nothing is written to browser storage.
/// Done and Dismiss remove the row optimistically and put it back with
/// [Copy.actionFailed] on failure (XC-04).
class NeedsAttentionModel extends ChangeNotifier {
  NeedsAttentionModel({
    required ApiClient api,
    this.refreshEvery = const Duration(minutes: 5),
  }) : _api = api;

  final ApiClient _api;

  /// How often the open tab reloads (S9 section 6 `[DEFAULT]`).
  final Duration refreshEvery;

  NaLoadState _state = NaLoadState.loading;
  List<NeedsAttentionItem> _items = const <NeedsAttentionItem>[];
  int _openCount = 0;
  String? _actionError;
  bool _inFlight = false;

  NaLoadState get state => _state;
  List<NeedsAttentionItem> get items => _items;

  /// The badge count from `open_count`, not `items.length` (paging).
  int get openCount => _openCount;

  /// The copy to show after a failed action, or null (XC-04).
  String? get actionError => _actionError;

  /// Loads the first page. Newest first (NA-01 AC1), sorted on the client too.
  Future<void> load() async {
    if (_inFlight) return;
    _inFlight = true;
    _state = NaLoadState.loading;
    notifyListeners();
    try {
      final page = await _api.listNeedsAttention();
      _items = [...page.items]
        ..sort((a, b) => b.createdAt.compareTo(a.createdAt));
      _openCount = page.openCount;
      _state = NaLoadState.ready;
    } on Object {
      _items = const <NeedsAttentionItem>[];
      _state = NaLoadState.failed;
    } finally {
      _inFlight = false;
    }
    notifyListeners();
  }

  /// Marks [item] done (API-NA-2).
  Future<void> resolve(NeedsAttentionItem item) =>
      _act(item, (id) => _api.resolveNeedsAttention(id));

  /// Dismisses [item] (API-NA-3).
  Future<void> dismiss(NeedsAttentionItem item) =>
      _act(item, (id) => _api.dismissNeedsAttention(id));

  Future<void> _act(
    NeedsAttentionItem item,
    Future<void> Function(String itemId) call,
  ) async {
    _actionError = null;
    final index = _items.indexWhere((i) => i.itemId == item.itemId);
    if (index < 0) return;
    // Remove the row at once; decrease the badge count on success.
    _items = [..._items]..removeAt(index);
    if (_openCount > 0) {
      _openCount -= 1;
    }
    notifyListeners();
    try {
      await call(item.itemId);
    } on Object {
      // Put the row back in its sorted place with a visible message (XC-04).
      _items = [..._items, item]
        ..sort((a, b) => b.createdAt.compareTo(a.createdAt));
      _openCount += 1;
      _actionError = Copy.actionFailed;
      notifyListeners();
    }
  }
}
