// Accessibility guideline tests for the six main screens (T-1111).
//
// Each screen must satisfy the Android and iOS tap-target guidelines, every
// tap target must carry a semantics label, and all text must meet the contrast
// guideline. A violation fails the test: there are no blanket skips, and a
// genuine violation is fixed in the same change rather than waived.

import 'package:flutter_test/flutter_test.dart';

import 'screens.dart';

void main() {
  setUpAll(loadAppFonts);

  testWidgets(
    'a11y_main_screens_meet_tap_target_label_and_contrast_guidelines',
    (tester) async {
      final handle = tester.ensureSemantics();

      for (final screen in goldenScreens()) {
        for (final width in goldenWidths) {
          final app = await screen.build();
          await pumpAppAt(tester, app, width: width.toDouble());
          final open = screen.open;
          if (open != null) {
            await open(tester);
          }

          final where = '${screen.name} at ${width}px';
          await expectLater(
            tester,
            meetsGuideline(androidTapTargetGuideline),
            reason: 'androidTapTargetGuideline: $where',
          );
          await expectLater(
            tester,
            meetsGuideline(iOSTapTargetGuideline),
            reason: 'iOSTapTargetGuideline: $where',
          );
          await expectLater(
            tester,
            meetsGuideline(labeledTapTargetGuideline),
            reason: 'labeledTapTargetGuideline: $where',
          );
          await expectLater(
            tester,
            meetsGuideline(textContrastGuideline),
            reason: 'textContrastGuideline: $where',
          );
        }
      }

      handle.dispose();
    },
  );
}
