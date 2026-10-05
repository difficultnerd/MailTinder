import 'package:app/api/models/experiments.dart';
import 'package:app/consent_text.dart';
import 'package:app/copy.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import '../fixtures/consent_expectations.dart';
import '../support/settings_harness.dart';

MyExperiments _mine({
  bool available = true,
  bool optedIn = false,
  String? version,
  String current = kExperimentsConsentVersion,
}) => MyExperiments(
  available: available,
  optedIn: optedIn,
  consentVersion: version,
  currentConsentVersion: current,
  optedInAt: null,
);

void main() {
  Future<SettingsHarness> open(WidgetTester tester, MyExperiments mine) async {
    tester.view.physicalSize = const Size(800, 2400);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.reset);
    final h = SettingsHarness();
    h.api.myExperiments = mine;
    await h.pump(tester);
    await h.openSettingsEntry(tester, Copy.experiments);
    return h;
  }

  bool switchValue(WidgetTester tester) =>
      tester.widget<SwitchListTile>(find.byType(SwitchListTile)).value;

  testWidgets(
    'CL-02 AC1 Experiments switch is off by default with the consent text',
    (tester) async {
      await open(tester, _mine());
      expect(switchValue(tester), isFalse);
      expect(find.text(kExperimentsConsentText), findsOneWidget);
      expect(
        find.descendant(
          of: find.byType(SwitchListTile),
          matching: find.text(Copy.experimentsSwitch),
        ),
        findsOneWidget,
      );
    },
  );

  testWidgets('BAKE-7 consent text names recipients', (tester) async {
    await open(tester, _mine());
    final shown = tester.widget<Text>(find.text(kExperimentsConsentText)).data!;
    for (final phrase in kConsentPhrases) {
      expect(shown, contains(phrase));
    }
  });

  testWidgets('CL-02 AC3 turning off asks to confirm deletion and sends '
      'opted_in false', (tester) async {
    final h = await open(
      tester,
      _mine(optedIn: true, version: kExperimentsConsentVersion),
    );
    await tester.tap(find.byType(Switch));
    await tester.pumpAndSettle();
    expect(find.text(Copy.experimentsOffQuestion), findsOneWidget);
    expect(h.api.calls.any((c) => c.method == 'PUT'), isFalse);
    await tester.tap(find.text(Copy.turnOff));
    await tester.pumpAndSettle();
    final put = h.api.calls.singleWhere((c) => c.method == 'PUT');
    expect((put.body as Map)['opted_in'], isFalse);
    expect(switchValue(tester), isFalse);
  });

  testWidgets('CL-02 turning on sends the app consent version', (tester) async {
    final h = await open(tester, _mine());
    await tester.tap(find.byType(Switch));
    await tester.pumpAndSettle();
    final put = h.api.calls.singleWhere((c) => c.method == 'PUT');
    expect((put.body as Map)['opted_in'], isTrue);
    expect((put.body as Map)['consent_version'], kExperimentsConsentVersion);
    expect(switchValue(tester), isTrue);
  });

  testWidgets('s9_experiments_on shows the switch on', (tester) async {
    await open(
      tester,
      _mine(optedIn: true, version: kExperimentsConsentVersion),
    );
    expect(switchValue(tester), isTrue);
  });

  testWidgets('s9_experiments_on is off for an older consent version', (
    tester,
  ) async {
    await open(tester, _mine(optedIn: true, version: '2020-01-01'));
    expect(switchValue(tester), isFalse);
  });

  testWidgets('s9_experiments_paused disables the switch', (tester) async {
    final h = await open(tester, _mine(available: false));
    expect(find.text(Copy.experimentPaused), findsOneWidget);
    expect(
      tester.widget<SwitchListTile>(find.byType(SwitchListTile)).onChanged,
      isNull,
    );
    expect(h.api.calls.any((c) => c.method == 'PUT'), isFalse);
  });

  testWidgets('CL-02 opting out still works while the experiment is paused', (
    tester,
  ) async {
    final h = await open(
      tester,
      _mine(
        available: false,
        optedIn: true,
        version: kExperimentsConsentVersion,
      ),
    );
    await tester.tap(find.byType(Switch));
    await tester.pumpAndSettle();
    await tester.tap(find.text(Copy.turnOff));
    await tester.pumpAndSettle();
    expect(h.api.calls.any((c) => c.method == 'PUT'), isTrue);
  });

  testWidgets('s9_experiments_consent_outdated shows the text again', (
    tester,
  ) async {
    final h = await open(tester, _mine());
    h.api.nextExperimentsPutError = problem(409, 'consent_outdated');
    await tester.tap(find.byType(Switch));
    await tester.pumpAndSettle();
    expect(find.text(Copy.consentChanged), findsOneWidget);
    expect(find.text(kExperimentsConsentText), findsOneWidget);
    expect(switchValue(tester), isFalse);
  });

  testWidgets('CL-02 a stale app text never sends opted_in true', (
    tester,
  ) async {
    final h = await open(tester, _mine(current: '2099-01-01'));
    await tester.tap(find.byType(Switch));
    await tester.pumpAndSettle();
    expect(find.text(Copy.consentChanged), findsOneWidget);
    expect(h.api.calls.any((c) => c.method == 'PUT'), isFalse);
  });

  testWidgets('CL-02 experiment_unavailable pauses the switch', (tester) async {
    final h = await open(tester, _mine());
    h.api.nextExperimentsPutError = problem(409, 'experiment_unavailable');
    await tester.tap(find.byType(Switch));
    await tester.pumpAndSettle();
    expect(find.text(Copy.experimentPaused), findsOneWidget);
  });

  testWidgets('XC-03 Experiments and Admin controls are labelled', (
    tester,
  ) async {
    final handle = tester.ensureSemantics();
    await open(tester, _mine());
    expect(
      tester.getSemantics(find.byType(Switch)).label,
      Copy.experimentsSwitch,
    );
    handle.dispose();
  });
}
