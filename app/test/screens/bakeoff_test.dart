import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:app/api/api_client.dart';
import 'package:app/api/fake_api_client.dart';
import 'package:app/api/models/bakeoff.dart';
import 'package:app/copy.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import '../support/settings_harness.dart';

BakeoffReport _fixture({int? labelled, int minCell = 20}) {
  final json =
      jsonDecode(File('test/fixtures/bakeoff_report.json').readAsStringSync())
          as Map<String, Object?>;
  json['labelled_swipes'] = labelled ?? json['labelled_swipes'];
  json['min_cell_size'] = minCell;
  return BakeoffReport.fromJson(json);
}

BakeoffReport _empty() {
  final json =
      jsonDecode(File('test/fixtures/bakeoff_report.json').readAsStringSync())
          as Map<String, Object?>;
  json['labelled_swipes'] = null;
  return BakeoffReport.fromJson(json);
}

ApiException _mixed() => ApiException(
  status: 409,
  code: 'versions_mixed',
  requestId: 'r1',
  problem: {
    'code': 'versions_mixed',
    'versions_present': {
      'input': ['1', '2'],
      'question': ['1', '3'],
      'price': ['1'],
    },
  },
);

void main() {
  Future<SettingsHarness> open(
    WidgetTester tester, {
    BakeoffReport? report,
    Object? firstError,
    List<SnapshotSummary> snapshots = const [],
  }) async {
    tester.view.physicalSize = const Size(1600, 6000);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.reset);
    final h = SettingsHarness(isAdmin: true);
    h.api.bakeoffReports.add(report ?? _fixture());
    h.api.nextBakeoffError = firstError;
    h.api.snapshots.addAll(snapshots);
    for (final s in snapshots) {
      h.api.snapshotReports[s.snapshotId] = report ?? _fixture();
    }
    await h.pump(tester);
    await h.openSettingsEntry(tester, Copy.bakeoffReport);
    return h;
  }

  Iterable<FakeCall> writes(SettingsHarness h) =>
      h.api.calls.where((c) => c.method != 'GET');

  SnapshotSummary snap(String id, String name) => SnapshotSummary(
    snapshotId: id,
    name: name,
    createdAt: DateTime.utc(2026, 10, 4),
    labelledSwipes: 8312,
  );

  testWidgets('CL-04 AC1 kill switch sends PATCH for that model only', (
    tester,
  ) async {
    final h = await open(tester);
    await tester.tap(find.widgetWithText(SwitchListTile, 'Jev'));
    await tester.pumpAndSettle();
    final call = writes(h).single;
    expect(call.method, 'PATCH');
    expect(call.path, '/api/v1/admin/experiments/classifier');
    expect(call.body, {
      'models': [
        {'model': 'jev', 'enabled': false},
      ],
    });
  });

  testWidgets('CL-04 AC2 report shows per-method figures', (tester) async {
    await open(tester);
    for (final text in [
      'Header rules',
      'Gemini',
      'Jev',
      'Accuracy',
      'Junk precision',
      'Junk recall',
      'False junk rate',
      'ECE',
      'Latency p50',
      'Cost per 1,000 messages (USD)',
      'Confusion',
      'Agreement',
      'Latency histogram',
      'Daily trend',
      '0.35',
      '0.042',
      '420 ms',
    ]) {
      expect(find.text(text), findsWidgets, reason: text);
    }
  });

  testWidgets('CL-04 AC3 rates show intervals and paired shows McNemar p', (
    tester,
  ) async {
    await open(tester);
    expect(find.text('0.870 (0.863 to 0.877), n 8,312'), findsOneWidget);
    expect(find.text('0.002, n 8,312'), findsOneWidget);
    expect(find.text('McNemar p'), findsOneWidget);
    expect(find.text('0.012'), findsOneWidget);
    expect(find.text('0.050 (0.040 to 0.060)'), findsOneWidget);
  });

  testWidgets('CL-04 AC4 suppressed cells show Too few to show', (
    tester,
  ) async {
    await open(tester);
    expect(find.text(Copy.tooFewToShow), findsWidgets);
    // A suppressed confusion count and paired block are not filled in.
    expect(find.text('8,312'), findsWidgets);
    expect(find.text('2,500'), findsOneWidget);
  });

  testWidgets('CL-04 AC5 versions_mixed prompts to pick one version', (
    tester,
  ) async {
    final h = await open(tester, firstError: _mixed());
    expect(find.text(Copy.pickOneVersion), findsOneWidget);
    expect(find.text('Header rules'), findsNothing);
    await tester.tap(find.byType(DropdownButtonFormField<String?>).first);
    await tester.pumpAndSettle();
    await tester.tap(find.text('2').last);
    await tester.pumpAndSettle();
    final gets = h.api.calls.where((c) => c.path == '/api/v1/admin/bakeoff');
    final last = gets.last.body! as Map;
    expect(last['input_version'], '2');
    expect(find.text(Copy.pickOneVersion), findsNothing);
  });

  testWidgets('CL-04 AC6 save snapshot sends name and query', (tester) async {
    final h = await open(tester);
    await tester.enterText(find.byType(TextField), '  Blog draft ');
    await tester.tap(find.text(Copy.saveSnapshot));
    await tester.pumpAndSettle();
    final call = writes(h).single;
    expect(call.method, 'POST');
    expect(call.path, '/api/v1/admin/bakeoff/snapshots');
    final body = call.body! as Map;
    expect(body['name'], 'Blog draft');
    expect((body['query'] as Map)['pool_versions'], false);
    expect((body['query'] as Map)['from'], isA<String>());
  });

  testWidgets('CL-04 AC7 Download CSV saves with the fixed file name', (
    tester,
  ) async {
    final h = await open(tester, snapshots: [snap('s1', 'Blog draft')]);
    await tester.tap(find.text(Copy.downloadCsv).first);
    await tester.pumpAndSettle();
    expect(h.browser.saved.single.fileName, 'bakeoff-report.csv');
    expect(h.browser.saved.single.mimeType, 'text/csv');
    expect(h.browser.saved.single.bytes, h.api.csvBytes);
    await tester.tap(find.text(Copy.downloadCsv).last);
    await tester.pumpAndSettle();
    expect(h.browser.saved.last.fileName, 'bakeoff-snapshot.csv');
  });

  testWidgets(
    'CL-04 AC8 kill switch without a fresh sign-in shows Confirm it\'s you',
    (tester) async {
      final h = await open(tester);
      h.api.nextAdminWriteError = problem(403, 'step_up_required');
      await tester.tap(find.widgetWithText(SwitchListTile, 'Gemini'));
      await tester.pump();
      await tester.pump();
      expect(find.text('Confirm it\'s you'), findsWidgets);
      expect(find.text(Copy.stepUpKillSwitch('gemini', false)), findsWidgets);
    },
  );

  testWidgets('s9_bakeoff_loading', (tester) async {
    tester.view.physicalSize = const Size(1600, 6000);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.reset);
    final h = SettingsHarness(isAdmin: true);
    h.api.bakeoffReports.add(_fixture());
    final gate = Completer<void>();
    h.api.bakeoffGate = gate.future;
    await h.pump(tester);
    await h.tapTab(tester, Copy.tabSettings);
    await tester.tap(find.text(Copy.bakeoffReport));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 500));
    expect(find.byType(CircularProgressIndicator), findsOneWidget);
    gate.complete();
    await tester.pumpAndSettle();
    expect(find.byType(CircularProgressIndicator), findsNothing);
  });

  testWidgets('s9_bakeoff_not_enough_data', (tester) async {
    await open(tester, report: _empty());
    expect(find.text(Copy.notEnoughData), findsOneWidget);
    expect(find.text(Copy.downloadCsv), findsNothing);
    expect(find.text(Copy.bakeoffFrom, findRichText: false), findsNothing);
    expect(find.textContaining(Copy.bakeoffFrom), findsOneWidget);
  });

  testWidgets('s9_bakeoff_snapshot_saved', (tester) async {
    await open(tester);
    expect(find.text(Copy.snapshotSaved), findsNothing);
    await tester.enterText(find.byType(TextField), 'First');
    await tester.tap(find.text(Copy.saveSnapshot));
    await tester.pumpAndSettle();
    expect(find.text(Copy.snapshotSaved), findsOneWidget);
    expect(find.text('First'), findsOneWidget);
  });

  testWidgets('s9_bakeoff_snapshot_limit shows Delete a snapshot first', (
    tester,
  ) async {
    final h = await open(tester);
    h.api.nextAdminWriteError = problem(409, 'snapshot_limit');
    await tester.enterText(find.byType(TextField), 'One more');
    await tester.tap(find.text(Copy.saveSnapshot));
    await tester.pumpAndSettle();
    expect(find.text(Copy.deleteSnapshotFirst), findsOneWidget);
    expect(find.text(Copy.snapshotSaved), findsNothing);
  });

  testWidgets('s9_bakeoff_delete_snapshot confirms', (tester) async {
    final h = await open(tester, snapshots: [snap('s1', 'Blog draft')]);
    await tester.tap(find.text(Copy.delete));
    await tester.pumpAndSettle();
    expect(
      find.text(Copy.deleteSnapshotQuestion('Blog draft')),
      findsOneWidget,
    );
    expect(writes(h), isEmpty);
    await tester.tap(find.text(Copy.delete).last);
    await tester.pumpAndSettle();
    final call = writes(h).single;
    expect(call.method, 'DELETE');
    expect(call.path, '/api/v1/admin/bakeoff/snapshots/s1');
    expect(find.text('Blog draft'), findsNothing);
  });

  testWidgets('s9_bakeoff_snapshot opens read-only', (tester) async {
    await open(tester, snapshots: [snap('s1', 'Blog draft')]);
    await tester.tap(find.text('Blog draft'));
    await tester.pumpAndSettle();
    expect(find.text('0.870 (0.863 to 0.877), n 8,312'), findsOneWidget);
    expect(find.text(Copy.saveSnapshot), findsNothing);
  });

  test('BakeoffReport.fromJson accepts nulls wherever the schema allows', () {
    final r = _empty();
    expect(r.labelledSwipes, isNull);
    final jev = r.models.last;
    expect(jev.validAnswers, isNull);
    expect(jev.junkPrecision.suppressed, isTrue);
    expect(jev.ece, isNull);
    expect(jev.p50Ms, isNull);
    expect(r.paired.last.mcnemarP, isNull);
    expect(r.paired.last.difference.suppressed, isTrue);
    expect(r.confusion.last.count, isNull);
    expect(r.models.first.ece, isNull);
    expect(r.models[1].calibrationBins.last.count, isNull);
    expect(r.models[1].errorRate.lower, isNull);
  });

  testWidgets('XC-03 bake-off controls are labelled', (tester) async {
    await open(tester, snapshots: [snap('s1', 'Blog draft')]);
    final handle = tester.ensureSemantics();
    for (final label in [
      Copy.saveSnapshot,
      Copy.downloadCsv,
      Copy.delete,
      Copy.bakeoffSnapshotName,
      Copy.bakeoffInputVersion,
      Copy.bakeoffQuestionVersion,
      'Gemini',
      'Jev',
    ]) {
      expect(find.bySemanticsLabel(RegExp(label)), findsWidgets, reason: label);
    }
    handle.dispose();
  });
}
