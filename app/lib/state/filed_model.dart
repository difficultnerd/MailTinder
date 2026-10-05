import 'package:flutter/foundation.dart' show ChangeNotifier;

import '../api/api_client.dart';
import '../api/models/category.dart';
import '../api/models/feed.dart';
import '../copy.dart';

/// Loading states shared by the Filed tab and a category's message list (S9
/// section 5).
enum LoadState { loading, ready, failed }

/// Drives the Filed tab: the category list, rename and delete (S9 section 5,
/// API-CAT-1, API-CAT-3, API-CAT-4).
///
/// The list lives in memory only; nothing is written to browser storage.
class FiledModel extends ChangeNotifier {
  FiledModel({required ApiClient api}) : _api = api;

  final ApiClient _api;

  LoadState _state = LoadState.loading;
  List<Category> _categories = const <Category>[];

  LoadState get state => _state;
  List<Category> get categories => _categories;

  /// Opens the tab: one `listCategories` call.
  Future<void> load() async {
    _state = LoadState.loading;
    notifyListeners();
    try {
      final loaded = await _api.listCategories();
      _categories = loaded;
      _state = LoadState.ready;
    } on Object {
      _categories = const <Category>[];
      _state = LoadState.failed;
    }
    notifyListeners();
  }

  /// Renames [c] to [newName]. Returns the error copy to show, or null on
  /// success. An invalid name is refused locally, without an API call.
  Future<String?> rename(Category c, String newName) async {
    final trimmed = newName.trim();
    if (!categoryNameIsValid(trimmed)) {
      return Copy.categoryNameRule;
    }
    try {
      final updated = await _api.renameCategory(c.categoryId, trimmed);
      _categories = [
        for (final existing in _categories)
          if (existing.categoryId == c.categoryId) updated else existing,
      ];
      notifyListeners();
      return null;
    } on ApiException catch (e) {
      if (e.code == 'category_exists') return Copy.categoryExists(trimmed);
      return Copy.actionFailed;
    } on Object {
      return Copy.actionFailed;
    }
  }

  /// Deletes [c]'s label. Messages are never deleted (S3 INV-5). Returns the
  /// error copy to show, or null on success.
  Future<String?> delete(Category c) async {
    try {
      await _api.deleteCategory(c.categoryId);
      _categories = _categories
          .where((existing) => existing.categoryId != c.categoryId)
          .toList(growable: false);
      notifyListeners();
      return null;
    } on Object {
      return Copy.actionFailed;
    }
  }
}

/// A category name is 1 to 100 characters after trimming and neither starts nor
/// ends with `/` (Gmail nesting, S7 5.5).
bool categoryNameIsValid(String name) {
  final trimmed = name.trim();
  return trimmed.isNotEmpty &&
      trimmed.length <= 100 &&
      !trimmed.startsWith('/') &&
      !trimmed.endsWith('/');
}

/// Drives one category's message list: first page, then load more
/// (API-CAT-5).
class CategoryMessagesModel extends ChangeNotifier {
  CategoryMessagesModel({required ApiClient api, required Category category})
    : _api = api,
      _category = category;

  final ApiClient _api;
  final Category _category;

  LoadState _state = LoadState.loading;
  List<FiledMessage> _messages = const <FiledMessage>[];
  List<MailboxError> _mailboxErrors = const <MailboxError>[];
  String? _nextCursor;
  bool _inFlight = false;

  LoadState get state => _state;
  List<FiledMessage> get messages => _messages;
  List<MailboxError> get mailboxErrors => _mailboxErrors;
  String? get nextCursor => _nextCursor;
  Category get category => _category;

  /// First page: cursor null.
  Future<void> loadFirst() async {
    if (_inFlight) return;
    _inFlight = true;
    _state = LoadState.loading;
    notifyListeners();
    try {
      final page = await _api.listCategoryMessages(_category.categoryId);
      _messages = page.messages;
      _mailboxErrors = page.mailboxErrors;
      _nextCursor = page.nextCursor;
      _state = LoadState.ready;
    } on Object {
      _messages = const <FiledMessage>[];
      _state = LoadState.failed;
    } finally {
      _inFlight = false;
    }
    notifyListeners();
  }

  /// Next page, appended. Does nothing without a cursor.
  Future<void> loadMore() async {
    final cursor = _nextCursor;
    if (cursor == null || _inFlight) return;
    _inFlight = true;
    try {
      final page = await _api.listCategoryMessages(
        _category.categoryId,
        cursor: cursor,
      );
      _messages = [..._messages, ...page.messages];
      _mailboxErrors = page.mailboxErrors;
      _nextCursor = page.nextCursor;
      notifyListeners();
    } on Object {
      // Keep what is already on screen; the next scroll tries again.
    } finally {
      _inFlight = false;
    }
  }
}
