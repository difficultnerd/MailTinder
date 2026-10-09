// Shared harness for the T-1111 golden and accessibility regression tests.
//
// Every screen is built from seeded fake data and a fixed clock (the
// `buildCard` / `NeedsAttentionItem` helpers below pin `receivedAt` /
// `createdAt`), and no network is used: the four fake clients in
// `test/support/` answer every call.
//
// Text is rendered with the test font's placeholder squares: `flutter_tester`
// runs with `--use-test-fonts` and a `FontLoader` cannot override it, so the
// goldens are layout-only. See docs/golden-tests.md.

import 'package:app/api/fake_api_client.dart';
import 'package:app/api/models/feed.dart';
import 'package:app/api/models/needs_attention.dart';
import 'package:app/api/models/session.dart';
import 'package:app/screens/feed/feed_screen.dart';
import 'package:app/screens/feed/filing_sheet.dart';
import 'package:app/screens/needs_attention/needs_attention_screen.dart';
import 'package:app/screens/request_invite/request_invite_screen.dart';
import 'package:app/screens/settings/app_scope.dart';
import 'package:app/screens/settings/settings_screen.dart';
import 'package:app/screens/sign_in/sign_in_screen.dart';
import 'package:app/state/categories_cache.dart';
import 'package:app/state/feed_model.dart';
import 'package:app/state/needs_attention_model.dart';
import 'package:app/state/play_prefs.dart';
import 'package:app/state/session_model.dart';
import 'package:app/state/sign_in_model.dart';
import 'package:app/state/step_up_controller.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart' show FontLoader, rootBundle;
import 'package:flutter_test/flutter_test.dart';

import '../support/cards.dart';
import '../support/fake_browser.dart';
import '../support/fake_connectivity.dart';

/// The three phone widths every golden is captured at (logical pixels).
const goldenWidths = <int>[360, 390, 430];

/// The logical capture height: a common phone viewport.
const goldenHeight = 800;

/// Attempts to load the app's bundled Roboto faces so goldens would show real
/// text. On the `flutter test` engine this has no effect (it runs with
/// `--use-test-fonts`), so the goldens are layout-only; see docs/golden-tests.md.
Future<void> loadAppFonts() async {
  final loader = FontLoader('Roboto');
  for (final asset in _fontAssets) {
    loader.addFont(rootBundle.load(asset));
  }
  await loader.load();
}

const _fontAssets = <String>[
  'assets/fonts/Roboto-Regular.ttf',
  'assets/fonts/Roboto-Medium.ttf',
  'assets/fonts/Roboto-Bold.ttf',
];

/// Pumps [widget] at [width] logical pixels with a 1.0 device pixel ratio so a
/// golden is one image pixel per logical pixel.
Future<void> pumpAppAt(
  WidgetTester tester,
  Widget widget, {
  required double width,
  int height = goldenHeight,
}) async {
  tester.view.physicalSize = Size(width, height.toDouble());
  tester.view.devicePixelRatio = 1.0;
  addTearDown(tester.view.reset);
  // A unique key forces a fresh element subtree on every pump: without it
  // pumpWidget reuses the previous State and its `initState` (where the Feed
  // and Needs Attention models load) never runs again.
  await tester.pumpWidget(KeyedSubtree(key: UniqueKey(), child: widget));
  await tester.pumpAndSettle();
}

/// One screen under test: a [name] for the golden file, the [build] that seeds
/// it, and an optional [open] interaction run after the first settle (used by
/// the filing sheet, which only appears as a modal route).
class GoldenScreen {
  const GoldenScreen({required this.name, required this.build, this.open});

  final String name;
  final Future<Widget> Function() build;
  final Future<void> Function(WidgetTester tester)? open;
}

/// The six main screens, in the order the task lists them.
List<GoldenScreen> goldenScreens() => const <GoldenScreen>[
  GoldenScreen(name: 'sign_in', build: _signIn),
  GoldenScreen(name: 'invite', build: _invite),
  GoldenScreen(name: 'feed', build: _feed),
  GoldenScreen(
    name: 'filing_sheet',
    build: _filingSheet,
    open: _openFilingSheet,
  ),
  GoldenScreen(name: 'needs_attention', build: _needsAttention),
  GoldenScreen(name: 'settings', build: _settings),
];

ThemeData _theme() => ThemeData(fontFamily: 'Roboto');

Widget _app(Widget home) => MaterialApp(theme: _theme(), home: home);

// --- Sign-in ---------------------------------------------------------------

Future<Widget> _signIn() async {
  final api = FakeApiClient()
    ..session = const Session(state: SessionState.anonymous);
  final browser = FakeBrowser();
  return _app(
    SignInScreen(
      model: SignInModel(api: api, browser: browser),
      browser: browser,
    ),
  );
}

// --- Request invite --------------------------------------------------------

Future<Widget> _invite() async {
  final api = FakeApiClient()
    ..session = const Session(
      state: SessionState.pendingInviteRequest,
      pendingInviteEmail: 'james@example.com',
    );
  final browser = FakeBrowser();
  final session = SessionModel(api: api);
  await session.refresh();
  return _app(
    RequestInviteScreen(
      session: session,
      api: api,
      signInModel: SignInModel(api: api, browser: browser),
    ),
  );
}

// --- Feed ------------------------------------------------------------------

Future<Widget> _feed() async {
  final api = FakeApiClient()
    ..session = const Session(
      state: SessionState.authenticated,
      user: SessionUser(userId: 'u1', isAdmin: false),
      mailboxes: [_jane],
    );
  final session = SessionModel(api: api);
  await session.refresh();
  final model = FeedModel(
    api: api,
    session: session,
    connectivity: FakeConnectivity(),
  );
  api.feedPages.add(pageOf(_corpus(), nextCursor: 'cursor-1'));
  final categories = CategoriesCache(api: api);
  return _app(
    Scaffold(
      body: FeedScreen(
        model: model,
        session: session,
        api: api,
        browser: FakeBrowser(),
        categories: categories,
      ),
    ),
  );
}

/// A fixed, synthetic Feed: the T-204 corpus shapes (list, bulk without a
/// header, notice, suspect) with `example.com` addresses only.
List<FeedCard> _corpus() => <FeedCard>[
  buildCard(
    messageId: 'msg-1',
    senderName: 'Acme Weekly',
    senderAddress: 'digest@example.com',
    subject: 'Your weekly deals are here',
    preview: 'Save 40% on everything in the spring sale.',
    bulkReason: 'Sent through a mailing list provider.',
    bulkScore: 82,
    messageClass: MessageClass.list,
    hasOneClick: true,
    suggestion: const Suggestion(
      categoryId: 'cat-deals',
      name: 'Deals',
      alternates: <CategoryRef>[
        CategoryRef(categoryId: 'cat-shop', name: 'Shopping'),
        CategoryRef(categoryId: 'cat-news', name: 'Newsletters'),
      ],
      confidence: SuggestionConfidence.suggested,
    ),
  ),
  buildCard(
    messageId: 'msg-2',
    senderName: 'ShopMart',
    senderAddress: 'offers@example.com',
    subject: 'Weekend offers just for you',
    preview: 'Free delivery on your next order.',
    bulkReason: 'No List-Unsubscribe header.',
    bulkScore: 74,
    messageClass: MessageClass.bulkNoHeader,
  ),
  buildCard(
    messageId: 'msg-3',
    senderName: 'Prize Winner',
    senderAddress: 'winner@example.com',
    subject: 'You have won a voucher',
    preview: 'Claim your prize within 24 hours.',
    bulkReason: 'Spoofed sender domain.',
    bulkScore: 96,
    messageClass: MessageClass.suspect,
  ),
  buildCard(
    messageId: 'msg-4',
    senderName: 'Bank Statements',
    senderAddress: 'statements@example.com',
    subject: 'Your monthly statement',
    preview: 'Your October statement is ready to view.',
    bulkReason: 'Transaction notice.',
    bulkScore: 14,
    messageClass: MessageClass.notice,
    keepPrompt: const CategoryRef(categoryId: 'cat-tax', name: 'Tax invoices'),
  ),
];

// --- Filing sheet ----------------------------------------------------------

Future<Widget> _filingSheet() async {
  final api = FakeApiClient();
  final card = buildCard(
    messageId: 'msg-1',
    senderName: 'Acme Weekly',
    senderAddress: 'digest@example.com',
    subject: 'Your weekly deals are here',
    suggestion: const Suggestion(
      categoryId: 'cat-deals',
      name: 'Receipts',
      alternates: <CategoryRef>[
        CategoryRef(categoryId: 'cat-shop', name: 'Shopping'),
      ],
      confidence: SuggestionConfidence.learned,
    ),
  );
  final cache = CategoriesCache(api: api);
  return _app(
    Scaffold(
      body: Builder(
        builder: (context) => Center(
          child: ElevatedButton(
            onPressed: () => showFilingSheet(context, card, cache: cache),
            child: const Text('Open the filing sheet'),
          ),
        ),
      ),
    ),
  );
}

Future<void> _openFilingSheet(WidgetTester tester) async {
  await tester.tap(find.text('Open the filing sheet'));
  await tester.pumpAndSettle();
}

// --- Needs Attention -------------------------------------------------------

Future<Widget> _needsAttention() async {
  final api = FakeApiClient()
    ..session = const Session(
      state: SessionState.authenticated,
      user: SessionUser(userId: 'u1', isAdmin: false),
      mailboxes: [_jane, _bobSignedOut],
    );
  final session = SessionModel(api: api);
  await session.refresh();
  final model = NeedsAttentionModel(api: api);
  api.needsAttentionPages.add(
    NeedsAttentionPage(
      items: <NeedsAttentionItem>[
        _naItem(
          itemId: 'na-1',
          senderDisplay: 'Acme News',
          reason: NaReason.httpsOnlyUnsubscribe,
          link: 'https://unsub.example.com/acme',
          createdAt: DateTime.utc(2026, 10, 1, 9),
        ),
        _naItem(
          itemId: 'na-2',
          senderDisplay: 'Gadget Store',
          reason: NaReason.jobExpired,
          link: 'https://unsub.example.com/gadgets',
          createdAt: DateTime.utc(2026, 9, 28, 14),
        ),
        _naItem(
          itemId: 'na-3',
          mailboxId: 'mb-2',
          senderDisplay: 'Bob Weekly',
          reason: NaReason.mailboxNeedsSignIn,
          link: null,
          createdAt: DateTime.utc(2026, 9, 20, 8),
        ),
      ],
      openCount: 3,
      nextCursor: null,
    ),
  );
  return _app(
    Scaffold(
      body: NeedsAttentionScreen(
        model: model,
        session: session,
        api: api,
        browser: FakeBrowser(),
      ),
    ),
  );
}

NeedsAttentionItem _naItem({
  required String itemId,
  required String senderDisplay,
  required NaReason reason,
  required String? link,
  required DateTime createdAt,
  String mailboxId = 'mb-1',
}) {
  return NeedsAttentionItem(
    itemId: itemId,
    mailboxId: mailboxId,
    senderDisplay: senderDisplay,
    reason: reason,
    link: link == null ? null : Uri.parse(link),
    createdAt: createdAt,
  );
}

// --- Settings --------------------------------------------------------------

Future<Widget> _settings() async {
  final api = FakeApiClient()
    ..session = const Session(
      state: SessionState.authenticated,
      user: SessionUser(userId: 'u1', isAdmin: false),
      mailboxes: [_jane],
    );
  final browser = FakeBrowser();
  final session = SessionModel(api: api);
  await session.refresh();
  final stepUp = StepUpController(api: api, session: session, browser: browser);
  return _app(
    AppScope(
      session: session,
      api: api,
      browser: browser,
      stepUp: stepUp,
      playPrefs: PlayPrefs(),
      child: const SettingsScreen(),
    ),
  );
}

const _jane = Mailbox(
  mailboxId: 'mb-1',
  provider: 'gmail',
  emailAddress: 'jane@example.com',
  status: MailboxStatus.connected,
);

const _bobSignedOut = Mailbox(
  mailboxId: 'mb-2',
  provider: 'gmail',
  emailAddress: 'bob@example.com',
  status: MailboxStatus.needsSignIn,
);
