import 'package:app/api/api_client.dart';
import 'package:app/api/fake_api_client.dart';
import 'package:app/api/models/category.dart';
import 'package:app/api/models/feed.dart';
import 'package:app/api/models/session.dart';
import 'package:app/copy.dart';
import 'package:app/screens/feed/feed_screen.dart';
import 'package:app/screens/feed/filing_sheet.dart';
import 'package:app/screens/feed/keep_prompt.dart';
import 'package:app/screens/feed/swipeable_card.dart';
import 'package:app/state/categories_cache.dart';
import 'package:app/state/feed_model.dart';
import 'package:app/state/id_generator.dart';
import 'package:app/state/session_model.dart';
import 'package:app/state/swipe_controller.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import '../support/cards.dart';
import '../support/fake_browser.dart';
import '../support/fake_connectivity.dart';

const _jane = Mailbox(
  mailboxId: 'mb-1',
  provider: 'gmail',
  emailAddress: 'jane@example.com',
  status: MailboxStatus.connected,
);

Category _category(String id, String name, {int count = 0}) => Category(
  categoryId: id,
  name: name,
  messageCount: count,
  perMailbox: const <CategoryMailboxCount>[],
);

CategoryRef _ref(String id, String name) =>
    CategoryRef(categoryId: id, name: name);

Suggestion _suggestion({
  String? id = 'cat-1',
  String name = 'Tax invoices',
  List<CategoryRef> alternates = const <CategoryRef>[],
  SuggestionConfidence confidence = SuggestionConfidence.suggested,
}) => Suggestion(
  categoryId: id,
  name: name,
  alternates: alternates,
  confidence: confidence,
);

Finder _byLabel(String label) => find.byWidgetPredicate(
  (widget) => widget is Semantics && widget.properties.label == label,
);

/// Opens the filing sheet over a bare host and settles it.
Future<void> _openSheet(
  WidgetTester tester, {
  required FeedCard card,
  required FakeApiClient api,
  CategoriesCache? cache,
  NameProposer? proposer,
  void Function(FilingChoice?)? onResult,
  bool settle = true,
}) async {
  final categories = cache ?? CategoriesCache(api: api);
  await tester.pumpWidget(
    MaterialApp(
      home: Scaffold(
        body: Builder(
          builder: (context) => Center(
            child: ElevatedButton(
              onPressed: () async {
                final result = await showFilingSheet(
                  context,
                  card,
                  cache: categories,
                  proposer: proposer,
                );
                onResult?.call(result);
              },
              child: const Text('open sheet'),
            ),
          ),
        ),
      ),
    ),
  );
  await tester.tap(find.text('open sheet'));
  if (settle) {
    await tester.pumpAndSettle();
  } else {
    await tester.pump();
  }
}

class _FeedHarness {
  _FeedHarness() {
    api = FakeApiClient();
    api.session = const Session(
      state: SessionState.authenticated,
      user: SessionUser(userId: 'u1', isAdmin: false),
      mailboxes: [_jane],
    );
    browser = FakeBrowser();
    connectivity = FakeConnectivity();
    session = SessionModel(api: api);
    model = FeedModel(api: api, session: session, connectivity: connectivity);
    cache = CategoriesCache(api: api);
  }

  late final FakeApiClient api;
  late final FakeBrowser browser;
  late final FakeConnectivity connectivity;
  late final SessionModel session;
  late final FeedModel model;
  late final CategoriesCache cache;
}

Widget _feedApp(_FeedHarness h) => MaterialApp(
  home: Scaffold(
    body: FeedScreen(
      model: h.model,
      session: h.session,
      api: h.api,
      browser: h.browser,
      categories: h.cache,
    ),
  ),
);

Future<_FeedHarness> _pumpFeedCard(WidgetTester tester, FeedCard card) async {
  final h = _FeedHarness();
  h.api.feedPages.add(pageOf([card], nextCursor: 'c1'));
  await tester.pumpWidget(_feedApp(h));
  await tester.pumpAndSettle();
  return h;
}

List<FakeCall> _calls(_FeedHarness h, String path) =>
    h.api.calls.where((call) => call.path == path).toList();

void main() {
  testWidgets('SW-04 AC1 sheet shows suggestion first, two alternates and '
      'New category', (tester) async {
    final api = FakeApiClient();
    final card = buildCard(
      suggestion: _suggestion(
        alternates: [
          _ref('alt-1', 'Alt one'),
          _ref('alt-2', 'Alt two'),
          _ref('alt-3', 'Alt three'),
        ],
      ),
    );
    await _openSheet(tester, card: card, api: api);

    expect(find.text('Tax invoices'), findsOneWidget);
    expect(find.text('Alt one'), findsOneWidget);
    expect(find.text('Alt two'), findsOneWidget);
    // At most two alternates (S7 `suggestion`).
    expect(find.text('Alt three'), findsNothing);
    expect(find.text(Copy.newCategory), findsOneWidget);

    // The suggestion is above its alternates, which are above "New category".
    final suggestionY = tester.getTopLeft(find.text('Tax invoices')).dy;
    final alternateY = tester.getTopLeft(find.text('Alt one')).dy;
    final newCategoryY = tester.getTopLeft(find.text(Copy.newCategory)).dy;
    expect(suggestionY, lessThan(alternateY));
    expect(alternateY, lessThan(newCategoryY));
  });

  testWidgets('SW-04 AC3 no suggestion opens the name field', (tester) async {
    final api = FakeApiClient();
    final cache = CategoriesCache(api: api);
    // Loaded, with nothing in it: no categories yet.
    await cache.ensureLoaded();
    await _openSheet(tester, card: buildCard(), api: api, cache: cache);

    expect(find.byType(TextField), findsOneWidget);
    expect(find.text(Copy.newCategory), findsNothing);
  });

  testWidgets('FL-01 AC3 sheet choices render in the first frame without an '
      'API call', (tester) async {
    final api = FakeApiClient();
    final card = buildCard(suggestion: _suggestion());

    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: Builder(
            builder: (context) => Center(
              child: ElevatedButton(
                onPressed: () => showFilingSheet(
                  context,
                  card,
                  cache: CategoriesCache(api: api),
                ),
                child: const Text('open sheet'),
              ),
            ),
          ),
        ),
      ),
    );
    await tester.tap(find.text('open sheet'));
    await tester.pump();

    expect(find.text('Tax invoices'), findsOneWidget);
    expect(api.calls, isEmpty);
    await tester.pumpAndSettle();
  });

  testWidgets('FL-02 AC1 a new category name is sent as new_category_name', (
    tester,
  ) async {
    final h = await _pumpFeedCard(tester, buildCard());

    await tester.tap(find.byTooltip(Copy.fileButton));
    await tester.pumpAndSettle();
    await tester.enterText(find.byType(TextField), 'Tax invoices');
    await tester.pump();
    await tester.tap(
      find.descendant(
        of: find.byType(FilingSheet),
        matching: find.widgetWithText(FilledButton, Copy.fileButton),
      ),
    );
    await tester.pumpAndSettle();

    final swipe =
        _calls(h, '/api/v1/swipes').single.body! as Map<String, Object?>;
    expect(swipe['action'], 'file');
    expect(swipe['new_category_name'], 'Tax invoices');
    expect(swipe['category_id'], isNull);
  });

  testWidgets(
    'FL-03 AC1 learned sender shows File under with Other collapsed',
    (tester) async {
      final api = FakeApiClient();
      final card = buildCard(
        suggestion: _suggestion(
          name: 'Receipts',
          alternates: [_ref('alt-1', 'Alt one')],
          confidence: SuggestionConfidence.learned,
        ),
      );
      await _openSheet(tester, card: card, api: api);

      expect(find.text(Copy.fileUnder('Receipts')), findsOneWidget);
      expect(find.text(Copy.other), findsOneWidget);
      // Alternates stay collapsed until "Other" is tapped.
      expect(find.text('Alt one'), findsNothing);

      await tester.tap(find.text(Copy.other));
      await tester.pumpAndSettle();
      expect(find.text('Alt one'), findsOneWidget);
    },
  );

  testWidgets('FL-03 AC2 nothing is filed without a tap', (tester) async {
    final h = await _pumpFeedCard(
      tester,
      buildCard(
        suggestion: _suggestion(
          name: 'Receipts',
          confidence: SuggestionConfidence.learned,
        ),
      ),
    );

    await tester.tap(find.byTooltip(Copy.fileButton));
    await tester.pumpAndSettle();
    await tester.pump(const Duration(seconds: 10));

    expect(_calls(h, '/api/v1/swipes'), isEmpty);
    expect(find.text(Copy.fileUnder('Receipts')), findsOneWidget);
  });

  testWidgets('FL-04 AC1 keep prompt shows on a card with keep_prompt', (
    tester,
  ) async {
    await _pumpFeedCard(
      tester,
      buildCard(keepPrompt: _ref('cat-1', 'Receipts')),
    );

    expect(find.byType(KeepPrompt), findsOneWidget);
    expect(find.text(Copy.keepPrompt('Receipts')), findsOneWidget);
  });

  testWidgets('FL-04 AC2 accepting the keep prompt creates a file rule', (
    tester,
  ) async {
    final h = await _pumpFeedCard(
      tester,
      buildCard(keepPrompt: _ref('cat-1', 'Receipts')),
    );

    await tester.tap(
      find.descendant(
        of: find.byType(KeepPrompt),
        matching: find.text(Copy.fileButton),
      ),
    );
    await tester.pumpAndSettle();

    expect(h.api.lastFileRuleMailboxId, 'mb-1');
    expect(h.api.lastFileRuleMessageId, 'msg-1');
    expect(h.api.lastFileRuleCategoryId, 'cat-1');
    expect(_calls(h, '/api/v1/rules'), hasLength(1));
    // The card is filed too.
    final swipe =
        _calls(h, '/api/v1/swipes').single.body! as Map<String, Object?>;
    expect(swipe['action'], 'file');
    expect(swipe['category_id'], 'cat-1');
  });

  testWidgets('FL-04 AC2 dismissing the keep prompt sends nothing', (
    tester,
  ) async {
    final h = await _pumpFeedCard(
      tester,
      buildCard(keepPrompt: _ref('cat-1', 'Receipts')),
    );

    await tester.tap(
      find.descendant(
        of: find.byType(KeepPrompt),
        matching: find.byTooltip(Copy.dismiss),
      ),
    );
    await tester.pumpAndSettle();

    expect(find.text(Copy.keepPrompt('Receipts')), findsNothing);
    expect(_calls(h, '/api/v1/rules'), isEmpty);
    expect(_calls(h, '/api/v1/swipes'), isEmpty);
    // The card stays on the Feed.
    expect(find.byType(SwipeableCard), findsOneWidget);
  });

  testWidgets('s9_filing_sheet_loading_suggestion falls back after 200 ms', (
    tester,
  ) async {
    final api = FakeApiClient();
    // The cache never loads, so the sheet shows the loading state.
    await _openSheet(
      tester,
      card: buildCard(),
      api: api,
      cache: CategoriesCache(api: api),
      settle: false,
    );
    expect(find.byType(CircularProgressIndicator), findsOneWidget);

    await tester.pump(const Duration(milliseconds: 210));

    expect(find.byType(CircularProgressIndicator), findsNothing);
    expect(find.byType(TextField), findsOneWidget);
    await tester.pumpAndSettle();
  });

  testWidgets('s9_filing_sheet_no_categories goes straight to the name field', (
    tester,
  ) async {
    final api = FakeApiClient();
    final cache = CategoriesCache(api: api);
    await cache.ensureLoaded();
    await _openSheet(tester, card: buildCard(), api: api, cache: cache);

    expect(find.byType(TextField), findsOneWidget);
    expect(find.byType(CircularProgressIndicator), findsNothing);
  });

  testWidgets('s9_filing_sheet_name_exists selects the existing category', (
    tester,
  ) async {
    final api = FakeApiClient();
    api.categories.add(_category('cat-9', 'Receipts'));
    final cache = CategoriesCache(api: api);
    await cache.ensureLoaded();
    FilingChoice? result;
    await _openSheet(
      tester,
      card: buildCard(),
      api: api,
      cache: cache,
      onResult: (choice) => result = choice,
    );

    // The cached category is offered; "New category" opens the name field.
    await tester.tap(find.text(Copy.newCategory));
    await tester.pumpAndSettle();
    await tester.enterText(find.byType(TextField), ' receipts ');
    await tester.pump();
    await tester.tap(
      find.descendant(
        of: find.byType(FilingSheet),
        matching: find.widgetWithText(FilledButton, Copy.fileButton),
      ),
    );
    await tester.pumpAndSettle();

    expect(result?.categoryId, 'cat-9');
    expect(result?.newCategoryName, isNull);
  });

  testWidgets('s9_filing_sheet_cancel returns the card and sends nothing', (
    tester,
  ) async {
    final h = await _pumpFeedCard(tester, buildCard());

    await tester.tap(find.byTooltip(Copy.fileButton));
    await tester.pumpAndSettle();
    await tester.tap(find.text(Copy.cancel));
    await tester.pumpAndSettle();

    expect(_calls(h, '/api/v1/swipes'), isEmpty);
    expect(find.byType(SwipeableCard), findsOneWidget);
  });

  test(
    'category_exists on a new name resends once with the category id',
    () async {
      final api = FakeApiClient();
      api.categories.add(_category('cat-1', 'Receipts'));
      final session = SessionModel(api: api);
      final model = FeedModel(
        api: api,
        session: session,
        connectivity: FakeConnectivity(),
      );
      final cache = CategoriesCache(api: api);
      await cache.ensureLoaded();
      final card = buildCard(messageId: 'm1');
      api.feedPages.add(pageOf([card], nextCursor: 'c1'));
      await model.open();

      api.nextSwipeError = ApiException(
        status: 409,
        code: 'category_exists',
        requestId: 'r1',
      );
      final controller = SwipeController(
        api: api,
        feed: model,
        ids: IdGenerator(),
        categories: cache,
      );
      await controller.fileWith(
        card,
        const FilingChoice(newCategoryName: 'Receipts'),
      );

      final swipes = api.calls
          .where((call) => call.path == '/api/v1/swipes')
          .toList();
      expect(swipes, hasLength(2));
      final first = swipes[0].body! as Map<String, Object?>;
      expect(first['new_category_name'], 'Receipts');
      expect(first['category_id'], isNull);
      final second = swipes[1].body! as Map<String, Object?>;
      expect(second['category_id'], 'cat-1');
      expect(second['new_category_name'], isNull);
      // A fresh idempotency key for the resend.
      expect(second['idempotency_key'], isNot(first['idempotency_key']));
    },
  );

  testWidgets('XC-03 filing sheet controls are labelled', (tester) async {
    final api = FakeApiClient();
    final card = buildCard(
      suggestion: _suggestion(
        name: 'Receipts',
        alternates: [_ref('alt-1', 'Alt one')],
        confidence: SuggestionConfidence.learned,
      ),
    );
    await _openSheet(tester, card: card, api: api);

    expect(_byLabel(Copy.fileUnder('Receipts')), findsOneWidget);
    expect(_byLabel(Copy.other), findsOneWidget);
    expect(_byLabel(Copy.cancel), findsOneWidget);

    await tester.tap(find.text(Copy.other));
    await tester.pumpAndSettle();
    expect(_byLabel('Alt one'), findsOneWidget);
    expect(_byLabel(Copy.newCategory), findsOneWidget);
  });
}
