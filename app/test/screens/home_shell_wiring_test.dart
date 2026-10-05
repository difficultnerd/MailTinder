import 'package:app/screens/feed/card_view.dart';
import 'package:app/screens/feed/swipeable_card.dart';
import 'package:flutter_test/flutter_test.dart';

import '../support/cards.dart';
import '../support/settings_harness.dart';

void main() {
  testWidgets('the wired HomeShell shows real feed content on the Feed tab', (
    tester,
  ) async {
    final h = SettingsHarness();
    h.api.feedPages.add(
      pageOf([buildCard(subject: 'Wired subject')], nextCursor: 'c1'),
    );
    await h.pump(tester);

    expect(find.byType(SwipeableCard), findsOneWidget);
    expect(find.byType(CardView), findsWidgets);
    expect(find.text('Wired subject'), findsOneWidget);
    expect(h.api.calls.where((c) => c.path == '/api/v1/feed/next'), isNotEmpty);
  });
}
