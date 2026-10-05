import 'dart:async';

import 'package:app/platform/on_device_model.dart';
import 'package:app/state/name_proposer.dart';
import 'package:flutter_test/flutter_test.dart';

class FakeModel implements OnDeviceModel {
  FakeModel({this.available = true, this.reply, this.fail = false, this.hang});

  final bool available;
  final String? reply;
  final bool fail;
  final Completer<String?>? hang;
  String? lastPrompt;

  @override
  Future<bool> isAvailable() async => available;

  @override
  Future<String?> prompt(
    String text, {
    Duration timeout = const Duration(milliseconds: 800),
  }) async {
    lastPrompt = text;
    if (fail) throw StateError('model failed');
    final h = hang;
    if (h != null) return h.future.timeout(timeout);
    return reply;
  }
}

Future<String?> _propose(
  FakeModel model, {
  List<String> existing = const [],
  String subject = 'Your receipt',
}) => OnDeviceNameProposer(
  model,
).propose(senderDisplay: 'Shop', subject: subject, existing: existing);

void main() {
  test('FL-02 AC2 reply is cleaned and capped at 30 characters', () async {
    expect(await _propose(FakeModel(reply: '"receipts."\nextra')), 'Receipts');
    expect(
      await _propose(FakeModel(reply: '  travel   plans!  ')),
      'Travel Plans',
    );
    final long = await _propose(FakeModel(reply: 'a' * 50));
    expect(long?.length, 30);
    expect(await _propose(FakeModel(reply: 'me@example.com')), isNull);
    expect(await _propose(FakeModel(reply: 'https://x.test')), isNull);
    expect(await _propose(FakeModel(reply: '12345')), isNull);
    expect(await _propose(FakeModel(reply: '  ')), isNull);
    expect(
      await _propose(FakeModel(reply: 'receipts'), existing: ['Receipts']),
      'Receipts',
    );
    final model = FakeModel(reply: 'Bills');
    await _propose(model, subject: 's' * 200, existing: ['Taxes']);
    expect(model.lastPrompt, contains('s' * 120));
    expect(model.lastPrompt, isNot(contains('s' * 121)));
    expect(model.lastPrompt, contains('Taxes'));
  });

  test('FL-02 AC2 unavailable model returns null', () async {
    expect(await _propose(FakeModel(available: false, reply: 'X')), isNull);
  });

  test('FL-02 AC2 model error or timeout returns null', () async {
    expect(await _propose(FakeModel(fail: true)), isNull);
    final hang = Completer<String?>();
    final proposer = OnDeviceNameProposer(
      FakeModel(hang: hang),
      timeout: const Duration(milliseconds: 10),
    );
    expect(
      await proposer.propose(senderDisplay: 'a', subject: 'b', existing: []),
      isNull,
    );
  });
}
