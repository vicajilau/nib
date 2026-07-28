import 'package:flutter_test/flutter_test.dart';
import 'package:nib_client/main.dart';

/// Smoke test confirming the root `NibApp` widget builds without errors.
void main() {
  testWidgets('App loads cleanly', (WidgetTester tester) async {
    await tester.pumpWidget(const NibApp());
    expect(find.byType(NibApp), findsOneWidget);
  });
}
