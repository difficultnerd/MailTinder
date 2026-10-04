import 'dart:io';
import 'package:flutter_test/flutter_test.dart';

void main() {
  test('ASVS V14.3.3 pubspec has no browser storage packages', () {
    final pubspecFile = File('pubspec.yaml');
    expect(
      pubspecFile.existsSync(),
      isTrue,
      reason: 'pubspec.yaml should exist in current working directory',
    );

    final content = pubspecFile.readAsStringSync();

    const bannedPackages = [
      'shared_preferences',
      'hive',
      'sqflite',
      'idb_shim',
      'flutter_secure_storage',
    ];

    for (final package in bannedPackages) {
      final pattern = RegExp('^\\s*$package:', multiLine: true);
      expect(
        pattern.hasMatch(content),
        isFalse,
        reason:
            'pubspec.yaml must not contain banned storage package: $package',
      );
    }
  });
}
