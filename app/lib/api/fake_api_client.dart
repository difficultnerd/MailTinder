import 'dart:async';

import 'api_client.dart';
import 'models/auth.dart';
import 'models/category.dart';
import 'models/feed.dart';
import 'models/session.dart';
import 'models/swipe.dart';

class FakeCall {
  FakeCall(this.method, this.path, [this.body]);

  final String method;
  final String path;
  final Object? body;
}

class FakeApiClient implements ApiClient {
  final List<FakeCall> calls = [];
  Session session = Session.anonymous();
  Object? nextError; // thrown (and cleared) by the next call when set
  Uri authUrl = Uri.parse('https://accounts.google.com/o/oauth2/auth');
  bool inviteRequestSucceeds = true;

  /// Pages returned by [feedNext], consumed in order. When empty, [feedNext]
  /// returns an empty page with no cursor.
  final List<FeedPage> feedPages = [];

  /// When set, [feedNext] awaits it first: lets a test hold the Feed in a
  /// `loading` state or sequence requests.
  Completer<void>? feedGate;

  /// Results returned by [swipe], consumed in order. When empty, [swipe]
  /// returns a default `kept` result.
  final List<SwipeResult> swipeResults = [];

  /// When set, [swipe] throws it (and clears it) before returning.
  Object? nextSwipeError;

  /// When set, [swipe] awaits it first: lets a test hold a swipe in flight.
  Completer<void>? swipeGate;

  /// Results returned by [undo], consumed in order. When empty, [undo]
  /// returns a default restored result.
  final List<UndoResult> undoResults = [];

  /// When set, [undo] throws it (and clears it) before returning.
  Object? nextUndoError;

  /// When set, [createBlockRule] throws it (and clears it).
  Object? nextBlockRuleError;

  /// When set, [declineBlockPrompt] throws it (and clears it).
  Object? nextDeclineError;

  /// The promptRef passed to the last [createBlockRule] call.
  String? lastBlockedPromptRef;

  /// The promptRef passed to the last [declineBlockPrompt] call.
  String? lastDeclinedPromptRef;

  /// Categories returned by [listCategories]. [renameCategory] and
  /// [deleteCategory] update this list.
  final List<Category> categories = [];

  /// When set, [listCategories] awaits it first: lets a test hold the Filed
  /// tab in a `loading` state.
  Completer<void>? categoriesGate;

  /// When set, [listCategories] throws it (and clears it).
  Object? nextListCategoriesError;

  /// When set, [renameCategory] throws it (and clears it).
  Object? nextRenameError;

  /// When set, [deleteCategory] throws it (and clears it).
  Object? nextDeleteError;

  /// When set, [listCategoryMessages] throws it (and clears it).
  Object? nextCategoryMessagesError;

  /// Pages returned by [listCategoryMessages], consumed in order. When empty,
  /// an empty page with no cursor is returned.
  final List<FiledMessagePage> categoryMessagePages = [];

  /// The arguments of the last [renameCategory] call.
  String? lastRenamedCategoryId;
  String? lastRenamedName;

  /// The category id passed to the last [deleteCategory] call.
  String? lastDeletedCategoryId;

  @override
  Future<Session> getSession() async {
    calls.add(FakeCall('GET', '/api/v1/session'));
    if (nextError != null) {
      final err = nextError;
      nextError = null;
      throw err!;
    }
    return session;
  }

  @override
  Future<void> signOut() async {
    calls.add(FakeCall('POST', '/api/v1/auth/sign-out'));
    if (nextError != null) {
      final err = nextError;
      nextError = null;
      throw err!;
    }
  }

  @override
  Future<Uri> startAuth({
    required AuthIntent intent,
    String? inviteToken,
    String? mailboxId,
  }) async {
    calls.add(
      FakeCall('POST', '/api/v1/auth/google/start', {
        'intent': intent.wire,
        'invite_token': inviteToken,
        'mailbox_id': mailboxId,
      }),
    );
    if (nextError != null) {
      final err = nextError;
      nextError = null;
      throw err!;
    }
    return authUrl;
  }

  @override
  Future<void> createInviteRequest() async {
    calls.add(FakeCall('POST', '/api/v1/invite-requests', const {}));
    if (nextError != null) {
      final err = nextError;
      nextError = null;
      throw err!;
    }
    if (!inviteRequestSucceeds) {
      throw const NetworkException();
    }
  }

  @override
  Future<FeedPage> feedNext({
    String? cursor,
    int limit = 20,
    bool refresh = false,
  }) async {
    calls.add(
      FakeCall('POST', '/api/v1/feed/next', {
        'cursor': cursor,
        'limit': limit,
        'refresh': refresh,
      }),
    );
    final gate = feedGate;
    if (gate != null) {
      await gate.future;
    }
    if (nextError != null) {
      final err = nextError;
      nextError = null;
      throw err!;
    }
    if (feedPages.isEmpty) {
      return const FeedPage(
        cards: [],
        nextCursor: null,
        phase: 'new',
        phaseChanged: false,
        mailboxErrors: [],
        ruleActionsApplied: 0,
      );
    }
    return feedPages.removeAt(0);
  }

  @override
  Future<SwipeResult> swipe(
    SwipeRequest req, {
    required String idempotencyKey,
  }) async {
    calls.add(
      FakeCall('POST', '/api/v1/swipes', {
        ...req.toJson(),
        'idempotency_key': idempotencyKey,
      }),
    );
    final gate = swipeGate;
    if (gate != null) {
      await gate.future;
    }
    if (nextSwipeError != null) {
      final err = nextSwipeError;
      nextSwipeError = null;
      throw err!;
    }
    if (swipeResults.isNotEmpty) {
      return swipeResults.removeAt(0);
    }
    return const SwipeResult(
      outcome: 'kept',
      undoToken: 'undo-1',
      prompts: [],
      achievementsUnlocked: [],
      bossDefeated: false,
    );
  }

  @override
  Future<UndoResult> undo(String undoToken) async {
    calls.add(
      FakeCall('POST', '/api/v1/swipes/undo', {'undo_token': undoToken}),
    );
    if (nextUndoError != null) {
      final err = nextUndoError;
      nextUndoError = null;
      throw err!;
    }
    if (undoResults.isNotEmpty) {
      return undoResults.removeAt(0);
    }
    return const UndoResult(restored: true, unsubscribeAlreadySent: false);
  }

  @override
  Future<void> createBlockRule(String promptRef) async {
    calls.add(
      FakeCall('POST', '/api/v1/rules', {
        'kind': 'block_person',
        'prompt_ref': promptRef,
      }),
    );
    lastBlockedPromptRef = promptRef;
    if (nextBlockRuleError != null) {
      final err = nextBlockRuleError;
      nextBlockRuleError = null;
      throw err!;
    }
  }

  @override
  Future<void> declineBlockPrompt(String promptRef) async {
    calls.add(
      FakeCall('POST', '/api/v1/block-prompts/decline', {
        'prompt_ref': promptRef,
      }),
    );
    lastDeclinedPromptRef = promptRef;
    if (nextDeclineError != null) {
      final err = nextDeclineError;
      nextDeclineError = null;
      throw err!;
    }
  }

  /// Mailboxes returned by [listMailboxes]; [disconnectMailbox] removes from it.
  final List<Mailbox> mailboxes = [];

  /// When set, [disconnectMailbox] throws it (and clears it).
  Object? nextDisconnectError;

  /// When set, [deleteAccount] throws it (and clears it).
  Object? nextDeleteAccountError;

  /// Returned by [deleteAccount].
  DeleteAccountResult deleteResult = DeleteAccountResult(
    deletionDueBy: DateTime.utc(2026, 10, 6),
    appFoldersNotDeleted: const [],
  );

  @override
  Future<List<Mailbox>> listMailboxes() async {
    calls.add(FakeCall('GET', '/api/v1/mailboxes'));
    if (nextError != null) {
      final err = nextError;
      nextError = null;
      throw err!;
    }
    return List<Mailbox>.of(mailboxes);
  }

  @override
  Future<void> disconnectMailbox(String mailboxId) async {
    calls.add(FakeCall('DELETE', '/api/v1/mailboxes/$mailboxId'));
    if (nextDisconnectError != null) {
      final err = nextDisconnectError;
      nextDisconnectError = null;
      throw err!;
    }
    mailboxes.removeWhere((m) => m.mailboxId == mailboxId);
  }

  @override
  Future<DeleteAccountResult> deleteAccount() async {
    calls.add(FakeCall('DELETE', '/api/v1/account'));
    if (nextDeleteAccountError != null) {
      final err = nextDeleteAccountError;
      nextDeleteAccountError = null;
      throw err!;
    }
    return deleteResult;
  }

  @override
  Future<List<Category>> listCategories() async {
    calls.add(FakeCall('GET', '/api/v1/categories'));
    final gate = categoriesGate;
    if (gate != null) {
      await gate.future;
    }
    if (nextListCategoriesError != null) {
      final err = nextListCategoriesError;
      nextListCategoriesError = null;
      throw err!;
    }
    return List<Category>.unmodifiable(categories);
  }

  @override
  Future<Category> renameCategory(String categoryId, String name) async {
    calls.add(
      FakeCall('PATCH', '/api/v1/categories/$categoryId', {'name': name}),
    );
    lastRenamedCategoryId = categoryId;
    lastRenamedName = name;
    if (nextRenameError != null) {
      final err = nextRenameError;
      nextRenameError = null;
      throw err!;
    }
    final index = categories.indexWhere((c) => c.categoryId == categoryId);
    final updated = Category(
      categoryId: categoryId,
      name: name,
      messageCount: index >= 0 ? categories[index].messageCount : 0,
      perMailbox: index >= 0
          ? categories[index].perMailbox
          : const <CategoryMailboxCount>[],
    );
    if (index >= 0) {
      categories[index] = updated;
    }
    return updated;
  }

  @override
  Future<void> deleteCategory(String categoryId) async {
    calls.add(FakeCall('DELETE', '/api/v1/categories/$categoryId'));
    lastDeletedCategoryId = categoryId;
    if (nextDeleteError != null) {
      final err = nextDeleteError;
      nextDeleteError = null;
      throw err!;
    }
    categories.removeWhere((c) => c.categoryId == categoryId);
  }

  @override
  Future<FiledMessagePage> listCategoryMessages(
    String categoryId, {
    String? cursor,
    int limit = 20,
  }) async {
    calls.add(
      FakeCall('GET', '/api/v1/categories/$categoryId/messages', {
        'cursor': cursor,
        'limit': limit,
      }),
    );
    if (nextCategoryMessagesError != null) {
      final err = nextCategoryMessagesError;
      nextCategoryMessagesError = null;
      throw err!;
    }
    if (categoryMessagePages.isNotEmpty) {
      return categoryMessagePages.removeAt(0);
    }
    return const FiledMessagePage(
      messages: [],
      nextCursor: null,
      mailboxErrors: [],
    );
  }
}
