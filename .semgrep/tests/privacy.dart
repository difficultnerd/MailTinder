// Test cases for Semgrep privacy rules in .semgrep/privacy.yml

void testDartPrivacy(dynamic user) {
  // ruleid: privacy-dart-to-string-print-of-model
  print(user);

  // ok: privacy-dart-to-string-print-of-model
  debugPrint('loaded');
}
