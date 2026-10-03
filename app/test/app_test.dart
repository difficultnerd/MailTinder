import 'package:app/main.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  testWidgets('renders greeting', (tester) async {
    await tester.pumpWidget(const App());
    expect(find.text('Hello'), findsOneWidget);
  });
}
