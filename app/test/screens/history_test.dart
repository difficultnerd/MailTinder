import 'package:app/api/models/history.dart';
import 'package:app/api/models/rule.dart';
import 'package:app/copy.dart';
import 'package:app/format.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import '../support/settings_harness.dart';

HistoryEntry entry(
  String id,
  DateTime at, {
  String sender = 'Acme News',
  String? ruleId,
  String action = 'unsubscribe',
  String outcome = 'sent',
}) => HistoryEntry(
  entryId: id,
  at: at,
  mailboxId: 'm1',
  senderDisplay: sender,
  action: action,
  outcome: outcome,
  ruleId: ruleId,
);

Rule rule(String id, {int? yearly = 120, bool enabled = true}) => Rule(
  ruleId: id,
  kind: RuleKind.rejectList,
  match: const RuleMatch(senderAddress: 'news@acme.example', listId: null),
  categoryId: null,
  enabled: enabled,
  createdAt: DateTime.utc(2026, 9, 1),
  timesApplied: 3,
  yearlyRate: yearly,
);

void main() {
  Future<SettingsHarness> open(
    WidgetTester tester, {
    List<HistoryEntry> entries = const [],
    List<Rule> rules = const [],
    String? nextCursor,
  }) async {
    final h = SettingsHarness();
    h.api.mailboxes.add(mailbox('m1', 'me@example.com'));
    h.api.rules.addAll(rules);
    h.api.historyPages.add(
      HistoryPage(entries: entries, nextCursor: nextCursor),
    );
    await h.pump(tester);
    await h.openSettingsEntry(tester, Copy.history);
    return h;
  }

  testWidgets(
    'ST-01 AC1 History lists entries newest first with time, sender and mailbox',
    (tester) async {
      final newer = DateTime.utc(2026, 10, 3, 4, 5);
      final older = DateTime.utc(2026, 10, 1, 4, 5);
      await open(
        tester,
        entries: [
          entry('e1', newer, sender: 'Newer Sender'),
          entry(
            'e2',
            older,
            sender: 'Older Sender',
            outcome: 'needs_attention',
          ),
        ],
      );
      expect(find.text(formatDateTime(newer)), findsOneWidget);
      expect(find.text('me@example.com'), findsNWidgets(2));
      expect(find.text('Unsubscribe: Sent'), findsOneWidget);
      expect(find.text('Unsubscribe: Needs attention'), findsOneWidget);
      final newerY = tester.getTopLeft(find.text('Newer Sender')).dy;
      final olderY = tester.getTopLeft(find.text('Older Sender')).dy;
      expect(newerY, lessThan(olderY));
    },
  );

  testWidgets('ST-01 AC1 filters send the filter value', (tester) async {
    final h = await open(tester);
    await tester.tap(find.text(Copy.historyRuleActions));
    await tester.pumpAndSettle();
    await tester.tap(find.text(Copy.historyUnsubscribes));
    await tester.pumpAndSettle();
    await tester.tap(find.text(Copy.historyFiling));
    await tester.pumpAndSettle();
    final filters = [
      for (final c in h.api.calls.where((c) => c.path == '/api/v1/history'))
        (c.body! as Map)['filter'],
    ];
    expect(filters, ['all', 'rule_actions', 'unsubscribes', 'filing']);
  });

  testWidgets(
    'ST-01 AC2 a rule-driven entry opens its rule and the switch turns it off',
    (tester) async {
      final h = await open(
        tester,
        entries: [entry('e1', DateTime.utc(2026, 10, 3), ruleId: 'r1')],
        rules: [rule('r1')],
      );
      await tester.tap(find.text('Acme News'));
      await tester.pumpAndSettle();
      expect(find.text('news@acme.example'), findsOneWidget);
      await tester.tap(find.byType(Switch));
      await tester.pumpAndSettle();
      expect(h.api.rules.single.enabled, isFalse);
    },
  );

  testWidgets('SR-01 AC4 switching a rule off sends enabled false', (
    tester,
  ) async {
    final h = await open(
      tester,
      entries: [entry('e1', DateTime.utc(2026, 10, 3), ruleId: 'r1')],
      rules: [rule('r1')],
    );
    await tester.tap(find.text('Acme News'));
    await tester.pumpAndSettle();
    await tester.tap(find.byType(Switch));
    await tester.pumpAndSettle();
    final patch = h.api.calls.singleWhere((c) => c.method == 'PATCH');
    expect(patch.path, '/api/v1/rules/r1');
    expect((patch.body! as Map)['enabled'], false);
  });

  testWidgets('GM-05 AC2 History shows the per-sender yearly figure', (
    tester,
  ) async {
    await open(
      tester,
      entries: [entry('e1', DateTime.utc(2026, 10, 3), ruleId: 'r1')],
      rules: [rule('r1', yearly: 120)],
    );
    expect(find.text(Copy.yearlyStopped(120)), findsOneWidget);
  });

  testWidgets('GM-05 AC3 unknown yearly figure shows as unknown', (
    tester,
  ) async {
    await open(
      tester,
      entries: [entry('e1', DateTime.utc(2026, 10, 3), ruleId: 'r1')],
      rules: [rule('r1', yearly: null)],
    );
    expect(find.text(Copy.yearlyUnknown), findsOneWidget);
    expect(find.text(Copy.yearlyStopped(0)), findsNothing);
  });

  testWidgets('s9_history_empty', (tester) async {
    await open(tester);
    expect(find.text(Copy.historyEmpty), findsOneWidget);
  });

  testWidgets('XC-03 History, Rules and Stats controls are labelled', (
    tester,
  ) async {
    final handle = tester.ensureSemantics();
    final h = await open(
      tester,
      entries: [entry('e1', DateTime.utc(2026, 10, 3), ruleId: 'r1')],
      rules: [rule('r1')],
    );
    for (final label in [
      Copy.historyAll,
      Copy.historyUnsubscribes,
      Copy.historyRuleActions,
      Copy.historyFiling,
    ]) {
      expect(find.bySemanticsLabel(label), findsOneWidget);
    }
    await tester.tap(find.text('Acme News'));
    await tester.pumpAndSettle();
    expect(find.bySemanticsLabel(Copy.ruleOnSwitch), findsWidgets);
    await tester.tapAt(const Offset(5, 5));
    await tester.pumpAndSettle();
    await tester.pageBack();
    await tester.pumpAndSettle();
    await tester.tap(find.text(Copy.rules));
    await tester.pumpAndSettle();
    expect(find.byTooltip(Copy.delete), findsOneWidget);
    expect(find.byType(Switch), findsOneWidget);
    expect(h.api.rules, isNotEmpty);
    handle.dispose();
  });
}
