import 'package:app/api/fake_api_client.dart';
import 'package:app/api/models/feed.dart';
import 'package:app/state/feed_model.dart';
import 'package:app/state/session_model.dart';
import 'package:flutter_test/flutter_test.dart';

import '../support/cards.dart';
import '../support/fake_connectivity.dart';

void main() {
  group('FeedCard.fromJson', () {
    test('reads every S7 field and ignores unknown fields', () {
      final json = <String, Object?>{
        'mailbox_id': 'mb-1',
        'message_id': 'msg-9',
        'sender_name': 'Sender <b>Name</b>',
        'sender_address': 'sender@example.com',
        'subject': 'Hello <script>alert(1)</script>',
        'preview': '<b>x</b> example.com',
        'received_at': '2026-03-04T09:30:00Z',
        'bulk_score': 87,
        'bulk_reason': 'Sent through an email service.',
        'class': 'suspect',
        'unsubscribe_method': 'one_click',
        'has_one_click': true,
        'suggestion': {
          'category_id': 'cat-1',
          'name': 'Updates',
          'alternates': [
            {'category_id': 'cat-2', 'name': 'News'},
          ],
          'confidence': 'suggested',
        },
        'keep_prompt': {'category_id': 'cat-3', 'name': 'Receipts'},
        'skip_count': 1,
        'boss': {'remaining': 4},
        'provider_web_url': 'https://mail.google.com/',
        'classification_token': 'sealed-token',
        'classifier_id': 'header_rules@1',
        'unknown_field': 'ignored',
        'another_unknown': 42,
      };

      final card = FeedCard.fromJson(json);

      expect(card.mailboxId, 'mb-1');
      expect(card.messageId, 'msg-9');
      expect(card.senderName, 'Sender <b>Name</b>');
      expect(card.senderAddress, 'sender@example.com');
      expect(card.subject, 'Hello <script>alert(1)</script>');
      expect(card.preview, '<b>x</b> example.com');
      expect(card.receivedAt, DateTime.utc(2026, 3, 4, 9, 30));
      expect(card.bulkScore, 87);
      expect(card.bulkReason, 'Sent through an email service.');
      expect(card.messageClass, MessageClass.suspect);
      expect(card.hasOneClick, isTrue);
      expect(card.skipCount, 1);
      expect(card.providerWebUrl, Uri.parse('https://mail.google.com/'));
      expect(card.classificationToken, 'sealed-token');
      expect(card.classifierId, 'header_rules@1');

      final suggestion = card.suggestion;
      expect(suggestion, isNotNull);
      expect(suggestion!.categoryId, 'cat-1');
      expect(suggestion.name, 'Updates');
      expect(suggestion.confidence, SuggestionConfidence.suggested);
      expect(suggestion.alternates, hasLength(1));
      expect(suggestion.alternates.single.categoryId, 'cat-2');
      expect(suggestion.alternates.single.name, 'News');

      expect(card.keepPrompt, isNotNull);
      expect(card.keepPrompt!.categoryId, 'cat-3');
      expect(card.keepPrompt!.name, 'Receipts');

      expect(card.boss, isNotNull);
      expect(card.boss!.remaining, 4);
    });

    test('keeps optional fields null when absent', () {
      final card = buildCard();
      final json = {
        'cards': [
          {
            'mailbox_id': card.mailboxId,
            'message_id': card.messageId,
            'sender_name': card.senderName,
            'sender_address': card.senderAddress,
            'subject': card.subject,
            'preview': card.preview,
            'received_at': '2026-01-01T12:00:00Z',
            'bulk_score': 0,
            'bulk_reason': card.bulkReason,
            'class': 'bulk_no_header',
            'unsubscribe_method': 'none',
            'has_one_click': false,
            'suggestion': null,
            'keep_prompt': null,
            'skip_count': 0,
            'boss': null,
            'provider_web_url': 'https://mail.google.com/',
            'classification_token': 'tok-1',
          },
        ],
        'next_cursor': null,
        'phase': 'new',
        'phase_changed': false,
        'mailbox_errors': <Object?>[],
        'rule_actions_applied': 0,
      };
      final page = FeedPage.fromJson(json);
      expect(page.cards, hasLength(1));
      expect(page.cards.single.suggestion, isNull);
      expect(page.cards.single.keepPrompt, isNull);
      expect(page.cards.single.boss, isNull);
      expect(page.cards.single.classifierId, isNull);
      expect(page.nextCursor, isNull);
    });

    test('unknown message class throws FormatException, does not default', () {
      final json = {
        'cards': [
          {
            'mailbox_id': 'mb-1',
            'message_id': 'm',
            'sender_name': 'n',
            'sender_address': 'a@example.com',
            'subject': 's',
            'preview': 'p',
            'received_at': '2026-01-01T12:00:00Z',
            'bulk_score': 0,
            'bulk_reason': 'r',
            'class': 'mystery',
            'unsubscribe_method': 'none',
            'has_one_click': false,
            'suggestion': null,
            'keep_prompt': null,
            'skip_count': 0,
            'boss': null,
            'provider_web_url': 'https://mail.google.com/',
            'classification_token': 'tok',
          },
        ],
        'next_cursor': null,
        'phase': 'new',
        'phase_changed': false,
        'mailbox_errors': <Object?>[],
        'rule_actions_applied': 0,
      };
      expect(() => FeedPage.fromJson(json), throwsFormatException);
    });
  });

  group('FeedModel', () {
    FeedModel buildModel(
      FakeApiClient api, {
      SessionModel? session,
      FakeConnectivity? connectivity,
      int refillBelow = 5,
    }) {
      return FeedModel(
        api: api,
        session: session ?? SessionModel(api: api),
        connectivity: connectivity ?? FakeConnectivity(),
        refillBelow: refillBelow,
      );
    }

    test(
      'feed loads the next page when fewer than five cards remain',
      () async {
        final api = FakeApiClient();
        final model = buildModel(api);
        api.feedPages.addAll([
          pageOf([
            buildCard(messageId: 'a'),
            buildCard(messageId: 'b'),
            buildCard(messageId: 'c'),
          ], nextCursor: 'c1'),
          pageOf([
            buildCard(messageId: 'd'),
            buildCard(messageId: 'e'),
          ], nextCursor: 'c1'),
        ]);

        await model.open();

        final nextCalls = api.calls
            .where((c) => c.path == '/api/v1/feed/next')
            .toList();
        expect(nextCalls, hasLength(2));
        final secondBody = nextCalls[1].body as Map<String, Object?>;
        expect(secondBody['cursor'], 'c1');
        expect(secondBody['refresh'], isFalse);
        expect(model.status, FeedStatus.ready);
        expect(model.current, isA<CardItem>());
      },
    );

    test('feed stops after three empty pages', () async {
      final api = FakeApiClient();
      final model = buildModel(api);
      api.feedPages.addAll([
        emptyFeedPage(nextCursor: 'c1'),
        emptyFeedPage(nextCursor: 'c2'),
        emptyFeedPage(nextCursor: 'c3'),
      ]);

      await model.open();

      final nextCalls = api.calls
          .where((c) => c.path == '/api/v1/feed/next')
          .toList();
      // open's first page + two empty cascades, then it stops.
      expect(nextCalls, hasLength(3));
      expect(model.status, FeedStatus.empty);
      expect(model.current, isNull);
    });
  });
}
