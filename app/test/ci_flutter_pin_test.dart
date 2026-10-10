import 'dart:io';

import 'package:flutter_test/flutter_test.dart';

/// The green goldens are captured with one Flutter version, recorded in the CSP
/// decision (ADR 0001). CI must install that same version, not whatever
/// `channel: stable` resolves to, or a stable-channel bump can change
/// rasterisation and fail every golden for no code reason (T-1111a).
void main() {
  test('ci_pins_the_flutter_version_recorded_in_the_decision', () {
    final decision = File('../docs/decisions/0001-flutter-web-csp.md');
    expect(
      decision.existsSync(),
      isTrue,
      reason:
          'ADR 0001 must exist at ../docs/decisions/0001-flutter-web-csp.md',
    );

    final recorded = _decisionVersion(decision.readAsStringSync());
    expect(
      recorded,
      isNotNull,
      reason: 'ADR 0001 must record the Flutter version it was validated on',
    );

    final workflow = File('../.github/workflows/ci.yml');
    expect(
      workflow.existsSync(),
      isTrue,
      reason: 'CI workflow must exist at ../.github/workflows/ci.yml',
    );

    final pinned = _dartJobFlutterPin(workflow.readAsLinesSync());
    expect(
      pinned,
      recorded,
      reason:
          'the dart job must pin flutter-version to the version recorded in '
          'ADR 0001 ($recorded) so a stable-channel bump cannot fail the '
          'goldens',
    );
  });
}

/// The Flutter version after `Flutter:` in the CSP decision, e.g. `3.47.6`.
String? _decisionVersion(String decision) {
  final match = RegExp(
    r'^Flutter:\s*([0-9]+\.[0-9]+\.[0-9]+)',
    multiLine: true,
  ).firstMatch(decision);
  return match?.group(1);
}

/// The `flutter-version:` value set by the `subosito/flutter-action` step in
/// the workflow's `dart` job, or null when the job leaves it unpinned.
String? _dartJobFlutterPin(List<String> lines) {
  final jobHeader = RegExp(r'^  dart:\s*$');
  final jobStart = lines.indexWhere(jobHeader.hasMatch);
  if (jobStart < 0) {
    return null;
  }

  final nextJob = RegExp(r'^  [A-Za-z0-9_-]+:\s*$');
  var jobEnd = jobStart + 1;
  while (jobEnd < lines.length && !nextJob.hasMatch(lines[jobEnd])) {
    jobEnd++;
  }

  final actionStart = lines.indexWhere(
    (line) => line.contains('subosito/flutter-action'),
    jobStart,
  );
  if (actionStart < 0 || actionStart >= jobEnd) {
    return null;
  }

  // The step runs until the next `- ` at the step's six-space indentation.
  final nextStep = RegExp(r'^      - ');
  var actionEnd = actionStart + 1;
  while (actionEnd < jobEnd && !nextStep.hasMatch(lines[actionEnd])) {
    actionEnd++;
  }

  final versionLine = lines.indexWhere(
    (line) => line.trimLeft().startsWith('flutter-version:'),
    actionStart,
  );
  if (versionLine < 0 || versionLine >= actionEnd) {
    return null;
  }

  final value = lines[versionLine]
      .substring(lines[versionLine].indexOf(':') + 1)
      .trim();
  return value.isEmpty ? null : value;
}
