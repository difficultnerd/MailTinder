// Golden (screenshot) regression tests for the six main screens at the three
// phone widths the task names. T-1111.
//
// Run the whole suite with `flutter test`; regenerate the committed PNGs with
// `tools/update_goldens.sh` (`flutter test --update-goldens --tags golden`).
// A mismatch fails the test and leaves the diff images in
// `test/golden/failures/`.

@Tags(<String>['golden'])
library;

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
}
