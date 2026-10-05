import 'dart:async';

import 'package:app/api/api_client.dart';
import 'package:app/api/fake_api_client.dart';
import 'package:app/api/models/feed.dart';
import 'package:app/api/models/session.dart';
import 'package:app/api/models/swipe.dart';
import 'package:app/state/feed_model.dart';
import 'package:app/state/id_generator.dart';
import 'package:app/state/session_model.dart';
import 'package:app/state/swipe_controller.dart';
import 'package:flutter_test/flutter_test.dart';

import '../support/cards.dart';
import '../support/fake_connectivity.dart';

/// Deterministic id generator: sequential keys so tests can assert ordering
/// and key reuse.
class _SeqIds implements IdGenerator {
  int _n = 0;

  @override
  String uuidV4() => 'key-${_n++}';
}

class _Harness {
  _Harness() {
    api = FakeApiClient();
    api.session = const Session(
      state: SessionState.authenticated,
      user: SessionUser(userId: 'u1', isAdmin: false),
      mailboxes: [],
    );
    connectivity = FakeConnectivity(online: true);
    session = SessionModel(api: api);
    feed = FeedModel(api: api, session: session, connectivity: connectivity);
    ids = _SeqIds();
    controller = SwipeController(
      api: api,
      feed: feed,
      ids: ids,
      retryBase: const Duration(milliseconds: 1),
    );
  }

  late final FakeApiClient api;
  late final FakeConnectivity connectivity;
  late final SessionModel session;
  late final FeedModel feed;
  late final _SeqIds ids;
  late final SwipeController controller;

  Future<void> load(List<FeedCard> cards) async {
    api.feedPages.add(pageOf(cards));
    await session.refresh();
    await feed.open();
  }

  List<FakeCall> swipeCalls() =>
      api.calls.where((c) => c.path == '/api/v1/swipes').toList();

  List<FakeCall> undoCalls() =>
      api.calls.where((c) => c.path == '/api/v1/swipes/undo').toList();
}

SwipeResult _result(String outcome, {String undoToken = 'undo-1'}) {
  return SwipeResult(
    outcome: outcome,
    undoToken: undoToken,
    prompts: const [],
    achievementsUnlocked: const [],
    bossDefeated: false,
  );
}

void main() {
  test('SW-05 AC4 undo works back through every swipe in order', () async {
    final h = _Harness();
    await h.load([
      buildCard(messageId: 'a', senderName: 'A'),
      buildCard(messageId: 'b', senderName: 'B'),
      buildCard(messageId: 'c', senderName: 'C'),
    ]);
    h.api.swipeResults.addAll([
      _result('kept', undoToken: 'u-a'),
      _result('skipped', undoToken: 'u-b'),
      _result('trashed', undoToken: 'u-c'),
    ]);

    await h.controller.keep();
    await h.controller.skip();
    await h.controller.reject();
    expect(h.controller.canUndo, isTrue);

    await h.controller.undo();
    expect(h.undoCalls().last.body, {'undo_token': 'u-c'});
    expect(h.feed.current, isA<CardItem>());
    expect((h.feed.current! as CardItem).card.messageId, 'c');

    await h.controller.undo();
    expect(h.undoCalls().last.body, {'undo_token': 'u-b'});
    expect((h.feed.current! as CardItem).card.messageId, 'b');

    await h.controller.undo();
    expect(h.undoCalls().last.body, {'undo_token': 'u-a'});
    expect((h.feed.current! as CardItem).card.messageId, 'a');
    expect(h.controller.canUndo, isFalse);
  });

  test('swipes are sent one at a time in order', () async {
    final h = _Harness();
    await h.load([
      buildCard(messageId: 'a', senderName: 'A'),
      buildCard(messageId: 'b', senderName: 'B'),
    ]);
    h.api.swipeResults.addAll([_result('kept'), _result('skipped')]);
    h.api.swipeGate = Completer<void>();

    final first = h.controller.keep();
    final second = h.controller.skip();
    await Future<void>.delayed(Duration.zero);
    // Only the first swipe has been sent; the second waits on the chain.
    expect(h.swipeCalls(), hasLength(1));

    h.api.swipeGate!.complete();
    await first;
    await second;

    expect(h.swipeCalls(), hasLength(2));
    final actions = h.swipeCalls().map(
      (c) => (c.body as Map<String, Object?>)['action'],
    );
    expect(actions, ['keep', 'skip']);
  });

  test('undo waits for the swipe ack before sending', () async {
    final h = _Harness();
    await h.load([buildCard(messageId: 'a', senderName: 'A')]);
    h.api.swipeResults.addAll([_result('kept', undoToken: 'u-a')]);
    h.api.swipeGate = Completer<void>();

    final swipe = h.controller.keep();
    final undo = h.controller.undo();
    await Future<void>.delayed(Duration.zero);
    // Undo must not send before the swipe ack.
    expect(h.undoCalls(), isEmpty);

    h.api.swipeGate!.complete();
    await swipe;
    await undo;

    expect(h.undoCalls(), hasLength(1));
    expect(h.undoCalls().single.body, {'undo_token': 'u-a'});
  });

  test('network error retries with the same Idempotency-Key', () async {
    final h = _Harness();
    await h.load([buildCard(messageId: 'a', senderName: 'A')]);
    h.api.nextSwipeError = const NetworkException();

    await h.controller.keep();

    final calls = h.swipeCalls();
    expect(calls, hasLength(2));
    final key0 = (calls[0].body as Map<String, Object?>)['idempotency_key'];
    final key1 = (calls[1].body as Map<String, Object?>)['idempotency_key'];
    expect(key0, key1);
  });
}
