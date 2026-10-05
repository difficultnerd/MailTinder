import 'package:flutter/foundation.dart' show ChangeNotifier;

import '../api/models/admin.dart';
import 'filed_model.dart' show LoadState;

/// One cursor-paged admin list (S9 7.6). In memory only.
class PagedList<T> extends ChangeNotifier {
  PagedList(this._fetch);

  final Future<Paged<T>> Function(String? cursor) _fetch;

  LoadState _state = LoadState.loading;
  final List<T> _items = [];
  String? _next;
  bool _loadingMore = false;

  LoadState get state => _state;
  List<T> get items => List.unmodifiable(_items);
  bool get hasMore => _next != null;
  bool get loadingMore => _loadingMore;

  Future<void> load() async {
    _state = LoadState.loading;
    notifyListeners();
    try {
      final page = await _fetch(null);
      _items
        ..clear()
        ..addAll(page.items);
      _next = page.nextCursor;
      _state = LoadState.ready;
    } on Object {
      _state = LoadState.failed;
    }
    notifyListeners();
  }

  Future<void> loadMore() async {
    final cursor = _next;
    if (cursor == null || _loadingMore) {
      return;
    }
    _loadingMore = true;
    notifyListeners();
    try {
      final page = await _fetch(cursor);
      _items.addAll(page.items);
      _next = page.nextCursor;
    } on Object {
      _state = LoadState.failed;
    }
    _loadingMore = false;
    notifyListeners();
  }

  void removeWhere(bool Function(T) test) {
    _items.removeWhere(test);
    notifyListeners();
  }
}
