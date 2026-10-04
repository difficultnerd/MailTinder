import 'dart:convert';
import 'dart:io';

import 'package:flutter_test/flutter_test.dart';

/// Reads ../firebase.json (from the app/ working directory) and asserts the
/// shipped hosting headers. Pins the policy before it is changed.
Map<String, Object?> _firebase() {
  final file = File('../firebase.json');
  return jsonDecode(file.readAsStringSync()) as Map<String, Object?>;
}

Map<String, String> _headersFor(String source) {
  final hosting = _firebase()['hosting'] as Map<String, Object?>;
  final headers = hosting['headers'] as List<Object?>;
  for (final block in headers) {
    final b = block as Map<String, Object?>;
    if (b['source'] == source) {
      final result = <String, String>{};
      for (final h in b['headers'] as List<Object?>) {
        final header = h as Map<String, Object?>;
        result[header['key'] as String] = header['value'] as String;
      }
      return result;
    }
  }
  return {};
}

List<Map<String, Object?>> _rewrites() {
  final hosting = _firebase()['hosting'] as Map<String, Object?>;
  return (hosting['rewrites'] as List<Object?>).cast<Map<String, Object?>>();
}

void main() {
  final headers = _headersFor('**');
  final csp = headers['Content-Security-Policy'] ?? '';

  test('ASVS V3.4.1 hosting sends hsts one year with subdomains', () {
    final hsts = headers['Strict-Transport-Security'] ?? '';
    expect(hsts, contains('max-age=31536000'));
    expect(hsts, contains('includeSubDomains'));
  });

  test('ASVS V3.4.3 hosting csp is strict with trusted types', () {
    expect(csp, contains("object-src 'none'"));
    expect(csp, contains("base-uri 'none'"));
    expect(csp, contains("frame-ancestors 'none'"));
    expect(csp, contains("default-src 'none'"));
    expect(csp, contains("require-trusted-types-for 'script'"));
    expect(csp, contains('trusted-types'));
  });

  test('ASVS V3.4.3 script-src has no unsafe-eval inline or remote source', () {
    final scriptSrc =
        RegExp(r"script-src ([^;]+)").firstMatch(csp)?.group(1) ?? '';
    expect(scriptSrc, isNot(contains("'unsafe-eval'")));
    expect(scriptSrc, isNot(contains("'unsafe-inline'")));
    expect(scriptSrc, isNot(contains('http:')));
    expect(scriptSrc, isNot(contains('https:')));
    expect(scriptSrc, isNot(contains('*')));
  });

  test('ASVS V3.4.4 hosting sends nosniff', () {
    expect(headers['X-Content-Type-Options'], 'nosniff');
  });

  test('ASVS V3.4.5 hosting sends no-referrer', () {
    expect(headers['Referrer-Policy'], 'no-referrer');
  });

  test('ASVS V3.4.6 hosting csp forbids framing', () {
    expect(csp, contains("frame-ancestors 'none'"));
  });

  test('api rewrite precedes spa rewrite', () {
    final rewrites = _rewrites();
    final apiIndex = rewrites.indexWhere((r) => r['source'] == '/api/**');
    final spaIndex = rewrites.indexWhere((r) => r['source'] == '**');
    expect(apiIndex, isNot(-1));
    expect(spaIndex, isNot(-1));
    expect(apiIndex, lessThan(spaIndex));
    final api = rewrites[apiIndex]['run'] as Map<String, Object?>;
    expect(api['serviceId'], 'api');
    expect(api['region'], 'us-central1');
  });
}
