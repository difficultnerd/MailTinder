import 'dart:async';

import 'package:app/api/api_client.dart';
import 'package:app/api/fake_api_client.dart';
import 'package:app/api/models/category.dart';
import 'package:app/api/models/feed.dart';
import 'package:app/api/models/session.dart';
import 'package:app/copy.dart';
import 'package:app/format.dart';
import 'package:app/routes.dart';
import 'package:app/screens/filed/filed_screen.dart';
import 'package:app/state/filed_model.dart';
import 'package:app/state/session_model.dart';
import 'package:app/state/sign_in_model.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import '../support/fake_browser.dart';

const _jane = Mailbox(
  mailboxId: 'mb-1',
  provider: 'gmail',
  emailAddress: 'jane@example.com',
  status: MailboxStatus.connected,
);
const _bob = Mailbox(
  mailboxId: 'mb-2',
  provider: 'gmail',
  emailAddress: 'bob@example.com',
  status: MailboxStatus.connected,
);

Category _category({
  String categoryId = 'c1',
  String name = 'Receipts',
  int messageCount = 12431,
}) {
  return Category(
    categoryId: categoryId,
    name: name,
    messageCount: messageCount,
    perMailbox: const [
      CategoryMailboxCount(mailboxId: 'mb-1', messageCount: 12431),
    ],
  );
}

FiledMessage _message({
  String mailboxId = 'mb-1',
  String messageId = 'msg-1',
  String senderName = 'Acme Billing',
  String subject = 'Your invoice',
  DateTime? receivedAt,
  String providerWebUrl = 'https://mail.google.com/mail/u/0/#inbox/msg-1',
}) {
  return FiledMessage(
    mailboxId: mailboxId,
    messageId: messageId,
    senderName: senderName,
    subject: subject,
    receivedAt: receivedAt ?? DateTime.utc(2026, 10, 3, 9),
    providerWebUrl: Uri.parse(providerWebUrl),
  );
}

class _Harness {
  _Harness() {
    api = FakeApiClient();
    api.session = const Session(
      state: SessionState.authenticated,
      user: SessionUser(userId: 'u1', isAdmin: false),
      mailboxes: [_jane, _bob],
    );
    browser = FakeBrowser();
    session = SessionModel(api: api);
    signIn = SignInModel(api: api, browser: browser);
    model = FiledModel(api: api);
  }

  late final FakeApiClient api;
  late final FakeBrowser browser;
  late final SessionModel session;
  late final SignInModel signIn;
  late final FiledModel model;
}

/// Builds a harness and refreshes the session so `session.mailboxes` is
/// available for the mailbox badges.
Future<_Harness> _make({
  List<Category> categories = const [],
  List<FiledMessagePage> messagePages = const [],
  Completer<void>? categoriesGate,
}) async {
  final h = _Harness();
  h.api.categories.addAll(categories);
  h.api.categoryMessagePages.addAll(messagePages);
  h.api.categoriesGate = categoriesGate;
  await h.session.refresh();
  return h;
}

Widget _app(_Harness h) {
  return MaterialApp(
    onGenerateRoute: (settings) =>
        onGenerateRoute(settings, h.session, h.api, h.signIn, h.browser),
    home: Scaffold(
      body: FiledScreen(
        model: h.model,
        session: h.session,
        api: h.api,
        browser: h.browser,
      ),
    ),
  );
}

Future<void> _openMenu(WidgetTester tester, String name) async {
  await tester.tap(find.bySemanticsLabel(Copy.moreFor(name)));
  await tester.pumpAndSettle();
}

void main() {
  testWidgets('FL-05 AC1 Filed lists categories with counts', (tester) async {
    final h = await _make(categories: [_category()]);
    await tester.pumpWidget(_app(h));
    await tester.pumpAndSettle();

    expect(find.text('Receipts'), findsOneWidget);
    expect(find.text(formatCount(12431)), findsOneWidget);
    expect(find.text('12,431'), findsOneWidget);
  });

  testWidgets(
    'FL-05 AC1 tapping a category shows its messages with sender, subject, date and mailbox',
    (tester) async {
      final h = await _make(
        categories: [_category()],
        messagePages: [
          FiledMessagePage(
            messages: [_message()],
            nextCursor: null,
            mailboxErrors: const [],
          ),
        ],
      );
      await tester.pumpWidget(_app(h));
      await tester.pumpAndSettle();

      await tester.tap(find.text('Receipts'));
      await tester.pumpAndSettle();

      expect(find.text('Acme Billing'), findsOneWidget);
      expect(find.text('Your invoice'), findsOneWidget);
      expect(
        find.text(formatDate(DateTime.utc(2026, 10, 3, 9))),
        findsOneWidget,
      );
      expect(find.text('jane@example.com'), findsOneWidget);
    },
  );

  testWidgets('s9_filed_empty shows the start filing message', (tester) async {
    final h = await _make();
    await tester.pumpWidget(_app(h));
    await tester.pumpAndSettle();

    expect(find.text(Copy.filedEmpty), findsOneWidget);
  });

  testWidgets('s9_filed_loading shows a progress indicator', (tester) async {
    final gate = Completer<void>();
    final h = await _make(categories: [_category()], categoriesGate: gate);
    await tester.pumpWidget(_app(h));
    await tester.pump();

    expect(find.byType(CircularProgressIndicator), findsOneWidget);

    gate.complete();
    await tester.pumpAndSettle();
    expect(find.text('Receipts'), findsOneWidget);
  });

  testWidgets(
    's9_filed_provider_error_per_mailbox shows a banner and other messages',
    (tester) async {
      final h = await _make(
        categories: [_category()],
        messagePages: [
          FiledMessagePage(
            messages: [_message()],
            nextCursor: null,
            mailboxErrors: const [
              MailboxError(mailboxId: 'mb-2', code: 'provider_unavailable'),
            ],
          ),
        ],
      );
      await tester.pumpWidget(_app(h));
      await tester.pumpAndSettle();

      await tester.tap(find.text('Receipts'));
      await tester.pumpAndSettle();

      expect(
        find.text(Copy.filedMailboxUnavailable('bob@example.com')),
        findsOneWidget,
      );
      expect(find.text('Acme Billing'), findsOneWidget);
    },
  );

  testWidgets(
    'rename sends PATCH and shows the clash message on category_exists',
    (tester) async {
      final h = await _make(categories: [_category()]);
      h.api.nextRenameError = ApiException(
        status: 409,
        code: 'category_exists',
        requestId: 'req-1',
      );
      await tester.pumpWidget(_app(h));
      await tester.pumpAndSettle();

      await _openMenu(tester, 'Receipts');
      await tester.tap(find.widgetWithText(PopupMenuItem<String>, Copy.rename));
      await tester.pumpAndSettle();

      await tester.enterText(find.byType(TextField), 'Invoices');
      await tester.tap(find.widgetWithText(TextButton, Copy.rename));
      await tester.pumpAndSettle();

      expect(h.api.lastRenamedCategoryId, 'c1');
      expect(h.api.lastRenamedName, 'Invoices');
      expect(find.text(Copy.categoryExists('Invoices')), findsOneWidget);
    },
  );

  testWidgets('rename refuses a name starting with a slash', (tester) async {
    final h = await _make(categories: [_category()]);
    await tester.pumpWidget(_app(h));
    await tester.pumpAndSettle();

    await _openMenu(tester, 'Receipts');
    await tester.tap(find.widgetWithText(PopupMenuItem<String>, Copy.rename));
    await tester.pumpAndSettle();

    await tester.enterText(find.byType(TextField), '/Tax');
    await tester.tap(find.widgetWithText(TextButton, Copy.rename));
    await tester.pumpAndSettle();

    expect(find.text(Copy.categoryNameRule), findsOneWidget);
    expect(h.api.lastRenamedName, isNull);
    expect(h.api.calls.where((call) => call.method == 'PATCH'), isEmpty);
  });

  testWidgets('delete asks for confirmation and sends DELETE', (tester) async {
    final h = await _make(categories: [_category()]);
    await tester.pumpWidget(_app(h));
    await tester.pumpAndSettle();

    await _openMenu(tester, 'Receipts');
    await tester.tap(find.widgetWithText(PopupMenuItem<String>, Copy.delete));
    await tester.pumpAndSettle();

    expect(find.text(Copy.deleteCategoryQuestion('Receipts')), findsOneWidget);

    await tester.tap(find.widgetWithText(TextButton, Copy.delete));
    await tester.pumpAndSettle();

    expect(h.api.lastDeletedCategoryId, 'c1');
    expect(find.text('Receipts'), findsNothing);
  });

  testWidgets('tapping a message opens provider_web_url in a new tab', (
    tester,
  ) async {
    final message = _message();
    final h = await _make(
      categories: [_category()],
      messagePages: [
        FiledMessagePage(
          messages: [message],
          nextCursor: null,
          mailboxErrors: const [],
        ),
      ],
    );
    await tester.pumpWidget(_app(h));
    await tester.pumpAndSettle();

    await tester.tap(find.text('Receipts'));
    await tester.pumpAndSettle();

    await tester.tap(find.text('Acme Billing'));
    await tester.pumpAndSettle();

    expect(h.browser.openedExternal, [message.providerWebUrl]);
  });

  testWidgets('a non-https provider url is not opened', (tester) async {
    final h = await _make(
      categories: [_category()],
      messagePages: [
        FiledMessagePage(
          messages: [
            _message(providerWebUrl: 'http://mail.example.com/#inbox/msg-1'),
          ],
          nextCursor: null,
          mailboxErrors: const [],
        ),
      ],
    );
    await tester.pumpWidget(_app(h));
    await tester.pumpAndSettle();

    await tester.tap(find.text('Receipts'));
    await tester.pumpAndSettle();

    await tester.tap(find.text('Acme Billing'));
    await tester.pumpAndSettle();

    expect(h.browser.openedExternal, isEmpty);
  });

  testWidgets('XC-03 Filed controls are labelled', (tester) async {
    final handle = tester.ensureSemantics();
    final h = await _make(categories: [_category()]);
    await tester.pumpWidget(_app(h));
    await tester.pumpAndSettle();

    expect(find.bySemanticsLabel(Copy.moreFor('Receipts')), findsOneWidget);

    await _openMenu(tester, 'Receipts');
    expect(find.text(Copy.rename), findsWidgets);
    expect(find.text(Copy.delete), findsWidgets);

    handle.dispose();
  });
}
