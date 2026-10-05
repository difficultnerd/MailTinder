import 'package:app/api/models/category.dart';
import 'package:app/api/models/rule.dart';
import 'package:app/copy.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import '../support/settings_harness.dart';

Rule _rule(
  String id,
  RuleKind kind,
  String address, {
  String? listId,
  String? categoryId,
  int applied = 4,
}) => Rule(
  ruleId: id,
  kind: kind,
  match: RuleMatch(senderAddress: address, listId: listId),
  categoryId: categoryId,
  enabled: true,
  createdAt: DateTime.utc(2026, 9, 1),
  timesApplied: applied,
  yearlyRate: null,
);

void main() {
  Future<SettingsHarness> open(WidgetTester tester) async {
    final h = SettingsHarness();
    h.api.rules.addAll([
      _rule(
        'r1',
        RuleKind.rejectList,
        'news@acme.example',
        listId: 'acme.list',
      ),
      _rule('r2', RuleKind.blockPerson, 'pal@example.org', applied: 1),
      _rule('r3', RuleKind.file, 'bank@example.net', categoryId: 'c1'),
    ]);
    h.api.categories.add(
      const Category(
        categoryId: 'c1',
        name: 'Receipts',
        messageCount: 2,
        perMailbox: [],
      ),
    );
    await h.pump(tester);
    await h.openSettingsEntry(tester, Copy.rules);
    return h;
  }

  testWidgets('s9_rules_lists_three_kinds_with_counts', (tester) async {
    await open(tester);
    expect(find.text(Copy.rulesRejectList), findsOneWidget);
    expect(find.text(Copy.rulesBlockedPeople), findsOneWidget);
    expect(find.text(Copy.rulesFiling), findsOneWidget);
    expect(find.text('news@acme.example'), findsOneWidget);
    expect(find.text('acme.list'), findsOneWidget);
    expect(find.text('Receipts'), findsOneWidget);
    expect(find.text(Copy.actedOn(4)), findsNWidgets(2));
    expect(find.text(Copy.actedOn(1)), findsOneWidget);
  });

  testWidgets('s9_rules_delete_confirms_then_deletes', (tester) async {
    final h = await open(tester);
    await tester.tap(find.byTooltip(Copy.delete).first);
    await tester.pumpAndSettle();
    expect(find.text(Copy.deleteRuleQuestion), findsOneWidget);
    expect(h.api.calls.any((c) => c.method == 'DELETE'), isFalse);
    await tester.tap(find.text(Copy.delete));
    await tester.pumpAndSettle();
    expect(h.api.calls.where((c) => c.method == 'DELETE'), hasLength(1));
    expect(find.text('news@acme.example'), findsNothing);
  });

  testWidgets('s9_rules_switch_error_reverts', (tester) async {
    final h = await open(tester);
    h.api.nextSetRuleError = problem(500, 'internal');
    await tester.tap(find.byType(Switch).first);
    await tester.pumpAndSettle();
    expect(tester.widget<Switch>(find.byType(Switch).first).value, isTrue);
    expect(find.text(Copy.actionFailed), findsOneWidget);
  });
}
