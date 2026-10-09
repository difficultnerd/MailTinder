// Golden (screenshot) regression tests for the six main screens at the three
// phone widths the task names. T-1111.
//
// Run the whole suite with `flutter test`; regenerate the committed PNGs with
// `tools/update_goldens.sh` (`flutter test --update-goldens --tags golden`).
// A mismatch fails the test and leaves the diff images in
// `test/golden/failures/`.

@Tags(<String>['golden'])
library;

import 'dart:typed_data';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import 'screens.dart';

void main() {
  setUpAll(loadAppFonts);

  for (final screen in goldenScreens()) {
    for (final width in goldenWidths) {
      testWidgets(
        'golden_screens_match_at_three_widths ${screen.name} at ${width}px',
        (tester) async {
          final app = await screen.build();
          await pumpAppAt(tester, app, width: width.toDouble());
          final open = screen.open;
          if (open != null) {
            await open(tester);
          }

          await expectLater(
            find.byType(MaterialApp),
            matchesGoldenFile('goldens/${screen.name}_$width.png'),
          );
        },
      );
    }
  }

  // T-1111 AC `golden_test_fails_when_a_widget_changes`. Proves the golden
  // comparison is a control, not decoration: render the sign-in screen at 360
  // logical pixels and compare the captured raster against the committed
  // 360-pixel golden (the same rendering — it must match) and against the
  // committed 390-pixel golden for the same screen (a different rendering — the
  // comparison must reject it). A recording comparator captures the failure
  // instead of letting the framework fail the test, so the rejection can be
  // asserted while the run stays green. If the comparison ever stopped
  // detecting a change, the second expectation would see no failure and this
  // test would fail.
  testWidgets('golden_test_fails_when_a_widget_changes', (tester) async {
    // `--update-goldens` rewrites goldens instead of comparing them, which
    // would make the recorder write the 360-pixel raster over the 390-pixel
    // golden. This test asserts a comparison outcome, so it does nothing in
    // that mode.
    if (autoUpdateGoldenFiles) {
      return;
    }

    final delegate = goldenFileComparator;
    final recorder = _RecordingGoldenComparator(delegate);
    goldenFileComparator = recorder;
    addTearDown(() => goldenFileComparator = delegate);

    final screen = goldenScreens().firstWhere((s) => s.name == 'sign_in');
    final app = await screen.build();
    await pumpAppAt(tester, app, width: 360);

    final Object? same = await matchesGoldenFile(
      'goldens/sign_in_360.png',
    ).matchAsync(find.byType(MaterialApp));
    expect(
      same,
      isNull,
      reason: 'the committed golden for the same rendering must match',
    );

    final Object? changed = await matchesGoldenFile(
      'goldens/sign_in_390.png',
    ).matchAsync(find.byType(MaterialApp));
    expect(
      changed,
      isNotNull,
      reason: 'a changed widget must fail the golden comparison',
    );
    expect(
      recorder.failures,
      hasLength(1),
      reason: 'the comparator must report exactly the changed rendering',
    );
  });
}

/// A [GoldenFileComparator] that delegates to [inner] and records a comparison
/// failure instead of throwing it. `LocalFileComparator.compare` throws a
/// `FlutterError` on a mismatch, which the test framework reports as a failure;
/// recording it lets the AC test assert the rejection while keeping the run
/// green.
class _RecordingGoldenComparator extends GoldenFileComparator {
  _RecordingGoldenComparator(this.inner);

  final GoldenFileComparator inner;
  final List<Object> failures = <Object>[];

  @override
  Future<bool> compare(Uint8List imageBytes, Uri golden) async {
    try {
      return await inner.compare(imageBytes, golden);
    } catch (error) {
      failures.add(error);
      return false;
    }
  }

  @override
  Future<void> update(Uri golden, Uint8List imageBytes) =>
      inner.update(golden, imageBytes);

  @override
  Uri getTestUri(Uri key, int? version) => inner.getTestUri(key, version);
}
