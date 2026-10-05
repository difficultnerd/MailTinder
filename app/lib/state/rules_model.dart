import 'package:flutter/foundation.dart' show ChangeNotifier;

import '../api/api_client.dart';
import '../api/models/category.dart';
import '../api/models/rule.dart';
import 'filed_model.dart' show LoadState;

/// Drives Rules (S9 7.3) and the rule sheet opened from History. In memory
/// only.
class RulesModel extends ChangeNotifier {
  RulesModel({required ApiClient api}) : _api = api;

  final ApiClient _api;

  LoadState _state = LoadState.loading;
  List<Rule> _rules = const <Rule>[];
  Map<String, String> _categoryNames = const <String, String>{};

  LoadState get state => _state;
  List<Rule> get rules => _rules;

  Rule? ruleById(String? id) {
    if (id == null) {
      return null;
    }
    for (final r in _rules) {
      if (r.ruleId == id) {
        return r;
      }
    }
    return null;
  }

  List<Rule> ofKind(RuleKind kind) => [
    for (final r in _rules)
      if (r.kind == kind) r,
  ];

  /// The category name for a filing rule, or null when unknown.
  String? categoryName(Rule r) => _categoryNames[r.categoryId];

  /// Loads rules and, when [withCategories], the category names for filing
  /// rules. A category lookup failure leaves the names blank.
  Future<void> load({bool withCategories = false}) async {
    _state = LoadState.loading;
    notifyListeners();
    try {
      _rules = await _api.listRules();
      _state = LoadState.ready;
    } on Object {
      _rules = const <Rule>[];
      _state = LoadState.failed;
    }
    if (withCategories && _state == LoadState.ready) {
      try {
        final List<Category> cats = await _api.listCategories();
        _categoryNames = {for (final c in cats) c.categoryId: c.name};
      } on Object {
        _categoryNames = const <String, String>{};
      }
    }
    notifyListeners();
  }

  void _replace(Rule updated) {
    _rules = [
      for (final r in _rules)
        if (r.ruleId == updated.ruleId) updated else r,
    ];
    notifyListeners();
  }

  /// Optimistic switch. Returns false (after reverting) when the call fails.
  Future<bool> setEnabled(Rule rule, bool enabled) async {
    _replace(rule.copyWith(enabled: enabled));
    try {
      final saved = await _api.setRuleEnabled(rule.ruleId, enabled);
      _replace(saved);
      return true;
    } on Object {
      _replace(rule);
      return false;
    }
  }

  /// Returns false when the delete fails.
  Future<bool> delete(Rule rule) async {
    try {
      await _api.deleteRule(rule.ruleId);
      _rules = [
        for (final r in _rules)
          if (r.ruleId != rule.ruleId) r,
      ];
      notifyListeners();
      return true;
    } on Object {
      return false;
    }
  }
}
