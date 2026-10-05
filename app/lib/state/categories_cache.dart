import 'package:flutter/foundation.dart' show ChangeNotifier;

import '../api/api_client.dart';
import '../api/models/category.dart';

/// The in-memory category list for the filling sheet (T-1003).
///
/// Loaded once per session, in the background when the Feed opens, so an
/// up-swipe has the names ready without a round trip (FL-01 AC3). Nothing is
/// written to browser storage.
class CategoriesCache extends ChangeNotifier {
  CategoriesCache({required ApiClient api}) : _api = api;

  final ApiClient _api;

  List<Category>? _categories;
  bool _started = false;

  /// The loaded categories, or null until the first successful load.
  List<Category>? get categories => _categories;

  /// Loads the list once per session. Errors leave [categories] null.
  Future<void> ensureLoaded() async {
    if (_started) return;
    _started = true;
    try {
      _categories = await _api.listCategories();
    } on Object {
      _categories = null;
    }
    notifyListeners();
  }

  /// Re-fetches the list (used after `409 category_exists`). A failure keeps
  /// whatever was already loaded.
  Future<void> reload() async {
    try {
      _categories = await _api.listCategories();
    } on Object {
      // Keep the previous list.
    }
    _started = true;
    notifyListeners();
  }

  /// The category with this name, compared case-insensitively after trimming,
  /// or null.
  Category? findByName(String name) {
    final target = name.trim().toLowerCase();
    if (target.isEmpty) return null;
    for (final category in _categories ?? const <Category>[]) {
      if (category.name.trim().toLowerCase() == target) return category;
    }
    return null;
  }
}
