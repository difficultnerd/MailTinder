import 'package:flutter/material.dart';

import '../../api/models/history.dart';
import '../../api/models/rule.dart';
import '../../copy.dart';
import '../../format.dart';
import '../../state/filed_model.dart' show LoadState;
import '../../state/history_model.dart';
import '../../state/rules_model.dart';
import 'app_scope.dart';
import 'rule_sheet.dart';

/// S9 7.2: every automated action, filtered, linking to its rule.
class HistoryScreen extends StatefulWidget {
  const HistoryScreen({super.key});

  @override
  State<HistoryScreen> createState() => _HistoryScreenState();
}

class _HistoryScreenState extends State<HistoryScreen> {
  HistoryModel? _history;
  RulesModel? _rules;
  Map<String, String> _mailboxes = const {};

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    if (_history != null) {
      return;
    }
    final scope = AppScope.maybeOf(context)!;
    _history = HistoryModel(api: scope.api)..load();
    _rules = RulesModel(api: scope.api)..load();
    _loadMailboxes(scope);
  }

  Future<void> _loadMailboxes(AppScope scope) async {
    try {
      final boxes = await scope.api.listMailboxes();
      if (mounted) {
        setState(() {
          _mailboxes = {for (final m in boxes) m.mailboxId: m.emailAddress};
        });
      }
    } on Object {
      // Rows show without a mailbox address.
    }
  }

  @override
  void dispose() {
    _history?.dispose();
    _rules?.dispose();
    super.dispose();
  }

  static const _filters = {
    HistoryFilter.all: Copy.historyAll,
    HistoryFilter.unsubscribes: Copy.historyUnsubscribes,
    HistoryFilter.ruleActions: Copy.historyRuleActions,
    HistoryFilter.filing: Copy.historyFiling,
  };

  String _yearly(Rule rule) {
    final rate = rule.yearlyRate;
    return rate == null ? Copy.yearlyUnknown : Copy.yearlyStopped(rate);
  }

  Widget _row(HistoryEntry e) {
    final rule = _rules!.ruleById(e.ruleId);
    final mailbox = _mailboxes[e.mailboxId];
    return ListTile(
      title: Text(e.senderDisplay),
      subtitle: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(formatDateTime(e.at)),
          if (mailbox != null) Text(mailbox),
          Text(
            '${Copy.historyAction(e.action)}: ${Copy.historyOutcome(e.outcome)}',
          ),
          if (rule != null) Text(_yearly(rule)),
        ],
      ),
      trailing: rule == null ? null : const Icon(Icons.chevron_right),
      onTap: rule == null
          ? null
          : () => showRuleSheet(context, _rules!, rule.ruleId),
    );
  }

  Widget _body(HistoryModel h) {
    switch (h.state) {
      case LoadState.loading:
        return const Center(child: CircularProgressIndicator());
      case LoadState.failed:
        return Center(
          child: Column(
            mainAxisSize: MainAxisSize.min,
            children: [
              const Text(Copy.genericError),
              TextButton(onPressed: h.load, child: const Text(Copy.tryAgain)),
            ],
          ),
        );
      case LoadState.ready:
        if (h.entries.isEmpty) {
          return const Center(child: Text(Copy.historyEmpty));
        }
        return ListView.builder(
          itemCount: h.entries.length,
          itemBuilder: (context, i) {
            if (i >= h.entries.length - 3 && h.hasMore) {
              WidgetsBinding.instance.addPostFrameCallback((_) {
                h.loadMore();
              });
            }
            return _row(h.entries[i]);
          },
        );
    }
  }

  @override
  Widget build(BuildContext context) {
    final h = _history!;
    return Scaffold(
      appBar: AppBar(title: const Text(Copy.history)),
      body: ListenableBuilder(
        listenable: Listenable.merge([h, _rules!]),
        builder: (context, _) => Column(
          children: [
            Padding(
              padding: const EdgeInsets.all(8),
              child: SegmentedButton<HistoryFilter>(
                showSelectedIcon: false,
                segments: [
                  for (final f in _filters.entries)
                    ButtonSegment(value: f.key, label: Text(f.value)),
                ],
                selected: {h.filter},
                onSelectionChanged: (s) => h.load(s.first),
              ),
            ),
            Expanded(child: _body(h)),
          ],
        ),
      ),
    );
  }
}
