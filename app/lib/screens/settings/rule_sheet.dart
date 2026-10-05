import 'package:flutter/material.dart';

import '../../api/models/rule.dart';
import '../../copy.dart';
import '../../state/rules_model.dart';

/// Shows a rule's match with an on or off switch (ST-01 AC2).
Future<void> showRuleSheet(
  BuildContext context,
  RulesModel model,
  String ruleId,
) {
  return showModalBottomSheet<void>(
    context: context,
    builder: (sheetContext) => ListenableBuilder(
      listenable: model,
      builder: (context, _) {
        final rule = model.ruleById(ruleId);
        if (rule == null) {
          return const SizedBox.shrink();
        }
        return RuleSheet(
          rule: rule,
          onChanged: (on) async {
            final ok = await model.setEnabled(rule, on);
            if (!ok && context.mounted) {
              ScaffoldMessenger.of(
                context,
              ).showSnackBar(const SnackBar(content: Text(Copy.actionFailed)));
            }
          },
        );
      },
    ),
  );
}

class RuleSheet extends StatelessWidget {
  const RuleSheet({super.key, required this.rule, required this.onChanged});

  final Rule rule;
  final ValueChanged<bool> onChanged;

  @override
  Widget build(BuildContext context) {
    final listId = rule.match.listId;
    return SafeArea(
      child: Padding(
        padding: const EdgeInsets.all(16),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text(rule.match.senderAddress),
            if (listId != null) Text(listId),
            Text(Copy.actedOn(rule.timesApplied)),
            SwitchListTile(
              title: const Text(Copy.ruleOnSwitch),
              value: rule.enabled,
              onChanged: onChanged,
            ),
          ],
        ),
      ),
    );
  }
}
