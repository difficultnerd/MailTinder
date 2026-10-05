import 'dart:async';

import 'package:flutter/material.dart';

import '../../api/models/category.dart';
import '../../api/models/feed.dart';
import '../../copy.dart';
import '../../state/categories_cache.dart';
import '../../state/filed_model.dart' show categoryNameIsValid;
import '../../state/swipe_controller.dart' show FilingChoice;

/// The hook for an on-device model that proposes a category name (FL-02 AC2).
/// With no proposer (or no model) the name field starts empty.
typedef NameProposer = Future<String?> Function(FeedCard card);

/// Opens the filing sheet for [card] and returns the chosen filing, or null
/// when the sheet is cancelled (S9 section 4, SW-04).
///
/// The choices render from `card.suggestion` and [cache] on the first frame:
/// no `listCategories` call is made here (FL-01 AC3).
Future<FilingChoice?> showFilingSheet(
  BuildContext context,
  FeedCard card, {
  required CategoriesCache cache,
  NameProposer? proposer,
  Duration suggestionBudget = const Duration(milliseconds: 200),
}) {
  return showModalBottomSheet<FilingChoice>(
    context: context,
    builder: (_) => FilingSheet(
      card: card,
      cache: cache,
      proposer: proposer,
      suggestionBudget: suggestionBudget,
    ),
  );
}

/// The filing sheet: suggested category first, up to two alternates and
/// "New category" (SW-04 AC1), collapsing to one tap once a sender is learned
/// (FL-03 AC1), or going straight to the name field when there is nothing to
/// suggest (SW-04 AC3).
class FilingSheet extends StatefulWidget {
  const FilingSheet({
    super.key,
    required this.card,
    required this.cache,
    this.proposer,
    this.suggestionBudget = const Duration(milliseconds: 200),
  });

  final FeedCard card;
  final CategoriesCache cache;
  final NameProposer? proposer;

  /// How long the "loading suggestion" state may last before falling back to
  /// the name field (S9 section 4).
  final Duration suggestionBudget;

  @override
  State<FilingSheet> createState() => _FilingSheetState();
}

class _FilingSheetState extends State<FilingSheet> {
  final TextEditingController _name = TextEditingController();
  Timer? _budget;
  bool _budgetExpired = false;
  bool _naming = false;
  bool _expanded = false;
  String? _nameError;
  bool _userTyped = false;

  @override
  void initState() {
    super.initState();
    if (_suggestion == null && widget.cache.categories == null) {
      _budget = Timer(widget.suggestionBudget, _onBudgetExpired);
    }
    final proposer = widget.proposer;
    if (proposer != null) {
      unawaited(_prefill(proposer));
    }
  }

  @override
  void dispose() {
    _budget?.cancel();
    _name.dispose();
    super.dispose();
  }

  void _onBudgetExpired() {
    if (!mounted) return;
    setState(() {
      _budgetExpired = true;
    });
  }

  Future<void> _prefill(NameProposer proposer) async {
    try {
      final proposed = await proposer(widget.card);
      if (!mounted || proposed == null || _userTyped) return;
      setState(() {
        _name.text = proposed;
      });
    } on Object {
      // A missing proposal is not an error: the field stays empty.
    }
  }

  /// The card's usable suggestion, or null when there is nothing to show.
  Suggestion? get _suggestion {
    final suggestion = widget.card.suggestion;
    if (suggestion == null) return null;
    if (suggestion.confidence == SuggestionConfidence.none) return null;
    if (suggestion.name == null || suggestion.name!.isEmpty) return null;
    return suggestion;
  }

  @override
  Widget build(BuildContext context) {
    return SafeArea(
      child: SingleChildScrollView(
        child: Padding(
          padding: const EdgeInsets.all(16),
          child: AnimatedBuilder(
            animation: widget.cache,
            builder: (context, _) => Column(
              mainAxisSize: MainAxisSize.min,
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: _body(),
            ),
          ),
        ),
      ),
    );
  }

  List<Widget> _body() {
    if (_naming) return _nameField();
    final suggestion = _suggestion;
    if (suggestion != null) {
      return suggestion.confidence == SuggestionConfidence.learned
          ? _learned(suggestion)
          : _suggested(suggestion);
    }
    final cached = widget.cache.categories;
    if (cached == null && !_budgetExpired) {
      return [_loading()];
    }
    if (cached == null || cached.isEmpty) {
      // No categories yet: go straight to the name field (SW-04 AC3).
      return _nameField();
    }
    return _cached(cached);
  }

  Widget _loading() {
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: 32),
      child: Center(
        child: Semantics(
          label: Copy.loading,
          child: const CircularProgressIndicator(),
        ),
      ),
    );
  }

  /// A learned sender: one tap, alternates collapsed behind "Other"
  /// (FL-03 AC1).
  List<Widget> _learned(Suggestion suggestion) {
    final name = suggestion.name!;
    return [
      _categoryButton(
        label: Copy.fileUnder(name),
        categoryId: suggestion.categoryId,
        primary: true,
      ),
      if (!_expanded)
        _textButton(
          label: Copy.other,
          onPressed: () => setState(() => _expanded = true),
        )
      else ...[
        for (final alternate in suggestion.alternates) _alternate(alternate),
        _newCategoryButton(),
      ],
      _cancelButton(),
    ];
  }

  /// A suggestion with its alternates: at most two (SW-04 AC1).
  List<Widget> _suggested(Suggestion suggestion) {
    return [
      _categoryButton(
        label: suggestion.name!,
        categoryId: suggestion.categoryId,
        primary: true,
      ),
      for (final alternate in suggestion.alternates.take(2))
        _alternate(alternate),
      _newCategoryButton(),
      _cancelButton(),
    ];
  }

  /// No suggestion: the cached categories, most-used first, at most three.
  List<Widget> _cached(List<Category> cached) {
    final sorted = [...cached]
      ..sort((a, b) => b.messageCount.compareTo(a.messageCount));
    return [
      for (final category in sorted.take(3))
        _categoryButton(label: category.name, categoryId: category.categoryId),
      _newCategoryButton(),
      _cancelButton(),
    ];
  }

  Widget _alternate(CategoryRef alternate) {
    return _categoryButton(
      label: alternate.name,
      categoryId: alternate.categoryId,
    );
  }

  Widget _categoryButton({
    required String label,
    required String? categoryId,
    bool primary = false,
  }) {
    final id = categoryId;
    final onPressed = id == null || id.isEmpty
        ? null
        : () => Navigator.of(context).pop(FilingChoice(categoryId: id));
    final child = Text(label);
    return Padding(
      padding: const EdgeInsets.only(bottom: 8),
      child: Semantics(
        label: label,
        button: true,
        child: primary
            ? FilledButton(onPressed: onPressed, child: child)
            : OutlinedButton(onPressed: onPressed, child: child),
      ),
    );
  }

  Widget _newCategoryButton() {
    return Padding(
      padding: const EdgeInsets.only(bottom: 8),
      child: _textButton(
        label: Copy.newCategory,
        onPressed: () => setState(() => _naming = true),
      ),
    );
  }

  Widget _textButton({required String label, required VoidCallback onPressed}) {
    return Semantics(
      label: label,
      button: true,
      child: TextButton(onPressed: onPressed, child: Text(label)),
    );
  }

  Widget _cancelButton() {
    return _textButton(
      label: Copy.cancel,
      onPressed: () => Navigator.of(context).pop(),
    );
  }

  List<Widget> _nameField() {
    return [
      TextField(
        controller: _name,
        decoration: InputDecoration(
          hintText: Copy.categoryNameHint,
          errorText: _nameError,
        ),
        onChanged: (_) => _userTyped = true,
        onSubmitted: (_) => _submitName(),
      ),
      const SizedBox(height: 16),
      Semantics(
        label: Copy.fileButton,
        button: true,
        child: FilledButton(
          onPressed: _submitName,
          child: const Text(Copy.fileButton),
        ),
      ),
      const SizedBox(height: 8),
      _cancelButton(),
    ];
  }

  /// Validates the typed name and returns the choice: an existing category is
  /// selected rather than created (S9 section 4).
  void _submitName() {
    final name = _name.text.trim();
    if (!categoryNameIsValid(name)) {
      setState(() => _nameError = Copy.categoryNameRule);
      return;
    }
    final existing = widget.cache.findByName(name);
    Navigator.of(context).pop(
      existing != null
          ? FilingChoice(categoryId: existing.categoryId)
          : FilingChoice(newCategoryName: name),
    );
  }
}
