import 'package:flutter/material.dart';

import '../../api/models/rule.dart';
import '../../copy.dart';
import '../../state/filed_model.dart' show LoadState;
import '../../state/rules_model.dart';
import 'app_scope.dart';

/// S9 7.3: reject list, blocked people and filing rules, with switch and delete.
class RulesScreen extends StatefulWidget {
  const RulesScreen({super.key});

  @override
  State<RulesScreen> createState() => _RulesScreenState();
}

class _RulesScreenState extends State<RulesScreen> {
  RulesModel? _model;

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    if (_model != null) {
      return;
    }
    _model = RulesModel(api: AppScope.maybeOf(context)!.api)
      ..load(withCategories: true);
  }

  @override
  void dispose() {
    _model?.dispose();
    super.dispose();
  }

  void _toast(String text) {
    ScaffoldMessenger.of(context).showSnackBar(SnackBar(content: Text(text)));
  }

  Future<void> _toggle(Rule r, bool on) async {
    if (!await _model!.setEnabled(r, on)) {
      if (mounted) {
        _toast(Copy.actionFailed);
      }
    }
  }

  Future<void> _delete(Rule r) async {
    final ok = await showDialog<bool>(
      context: context,
      builder: (dialogContext) => AlertDialog(
        content: const Text(Copy.deleteRuleQuestion),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(dialogContext).pop(false),
            child: const Text(Copy.cancel),
          ),
          TextButton(
            onPressed: () => Navigator.of(dialogContext).pop(true),
            child: const Text(Copy.delete),
          ),
        ],
      ),
    );
    if (ok != true || !mounted) {
      return;
    }
    if (!await _model!.delete(r) && mounted) {
      _toast(Copy.actionFailed);
    }
  }

  Widget _row(RulesModel m, Rule r) {
    final listId = r.match.listId;
    final category = r.kind == RuleKind.file ? m.categoryName(r) : null;
    return ListTile(
      title: Text(r.match.senderAddress),
      subtitle: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          if (listId != null) Text(listId),
          if (category != null) Text(category),
          Text(Copy.actedOn(r.timesApplied)),
        ],
      ),
      trailing: Wrap(
        crossAxisAlignment: WrapCrossAlignment.center,
        children: [
          Switch(value: r.enabled, onChanged: (on) => _toggle(r, on)),
          IconButton(
            tooltip: Copy.delete,
            icon: const Icon(Icons.delete_outline),
            onPressed: () => _delete(r),
          ),
        ],
      ),
    );
  }

  Widget _section(RulesModel m, String title, RuleKind kind) {
    final rows = m.ofKind(kind);
    if (rows.isEmpty) {
      return const SizedBox.shrink();
    }
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Padding(
          padding: const EdgeInsets.fromLTRB(16, 16, 16, 4),
          child: Text(title, style: Theme.of(context).textTheme.titleMedium),
        ),
        for (final r in rows) _row(m, r),
      ],
    );
  }

  @override
  Widget build(BuildContext context) {
    final m = _model!;
    return Scaffold(
      appBar: AppBar(title: const Text(Copy.rules)),
      body: ListenableBuilder(
        listenable: m,
        builder: (context, _) {
          switch (m.state) {
            case LoadState.loading:
              return const Center(child: CircularProgressIndicator());
            case LoadState.failed:
              return Center(
                child: Column(
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    const Text(Copy.genericError),
                    TextButton(
                      onPressed: () => m.load(withCategories: true),
                      child: const Text(Copy.tryAgain),
                    ),
                  ],
                ),
              );
            case LoadState.ready:
              if (m.rules.isEmpty) {
                return const Center(child: Text(Copy.historyEmpty));
              }
              return ListView(
                children: [
                  _section(m, Copy.rulesRejectList, RuleKind.rejectList),
                  _section(m, Copy.rulesBlockedPeople, RuleKind.blockPerson),
                  _section(m, Copy.rulesFiling, RuleKind.file),
                ],
              );
          }
        },
      ),
    );
  }
}
