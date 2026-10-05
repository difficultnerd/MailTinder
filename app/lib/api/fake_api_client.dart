import 'dart:async';

import 'api_client.dart';
import 'models/admin.dart';
import 'models/auth.dart';
import 'models/category.dart';
import 'models/experiments.dart';
import 'models/feed.dart';
import 'models/history.dart';
import 'models/rule.dart';
import 'models/session.dart';
import 'models/stats.dart';
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

  /// When set, [createFileRule] throws it (and clears it).
  Object? nextFileRuleError;

  /// The arguments of the last [createFileRule] call.
  String? lastFileRuleMailboxId;
  String? lastFileRuleMessageId;
  String? lastFileRuleCategoryId;

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
  Future<void> createFileRule({
    required String mailboxId,
    required String messageId,
    required String categoryId,
  }) async {
    calls.add(
      FakeCall('POST', '/api/v1/rules', {
        'kind': 'file',
        'mailbox_id': mailboxId,
        'message_id': messageId,
        'category_id': categoryId,
      }),
    );
    lastFileRuleMailboxId = mailboxId;
    lastFileRuleMessageId = messageId;
    lastFileRuleCategoryId = categoryId;
    if (nextFileRuleError != null) {
      final err = nextFileRuleError;
      nextFileRuleError = null;
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

  /// Rules returned by [listRules]; [setRuleEnabled] and [deleteRule] update it.
  final List<Rule> rules = [];

  /// When set, the matching rules call throws it (and clears it).
  Object? nextListRulesError;
  Object? nextSetRuleError;
  Object? nextDeleteRuleError;

  /// History pages returned by [listHistory], consumed in order. When empty,
  /// [listHistory] returns an empty page.
  final List<HistoryPage> historyPages = [];

  /// When set, [listHistory] throws it (and clears it).
  Object? nextHistoryError;

  /// Returned by [getStats].
  Stats stats = const Stats(
    emailsTriaged: 0,
    sendersUnsubscribed: 0,
    unsubscribesConfirmed: 0,
    mailStoppedPerYear: 0,
    achievements: [],
  );

  /// When set, [getStats] throws it (and clears it).
  Object? nextStatsError;

  @override
  Future<List<Rule>> listRules({RuleKind? kind}) async {
    calls.add(FakeCall('GET', '/api/v1/rules', {'kind': kind?.wire}));
    if (nextListRulesError != null) {
      final err = nextListRulesError;
      nextListRulesError = null;
      throw err!;
    }
    return [
      for (final r in rules)
        if (kind == null || r.kind == kind) r,
    ];
  }

  @override
  Future<Rule> setRuleEnabled(String ruleId, bool enabled) async {
    calls.add(FakeCall('PATCH', '/api/v1/rules/$ruleId', {'enabled': enabled}));
    if (nextSetRuleError != null) {
      final err = nextSetRuleError;
      nextSetRuleError = null;
      throw err!;
    }
    final index = rules.indexWhere((r) => r.ruleId == ruleId);
    if (index < 0) {
      throw ApiException(status: 404, code: 'not_found', requestId: 'fake');
    }
    rules[index] = rules[index].copyWith(enabled: enabled);
    return rules[index];
  }

  @override
  Future<void> deleteRule(String ruleId) async {
    calls.add(FakeCall('DELETE', '/api/v1/rules/$ruleId'));
    if (nextDeleteRuleError != null) {
      final err = nextDeleteRuleError;
      nextDeleteRuleError = null;
      throw err!;
    }
    rules.removeWhere((r) => r.ruleId == ruleId);
  }

  @override
  Future<HistoryPage> listHistory({
    HistoryFilter filter = HistoryFilter.all,
    String? cursor,
    int limit = 20,
  }) async {
    calls.add(
      FakeCall('GET', '/api/v1/history', {
        'filter': filter.wire,
        'cursor': cursor,
        'limit': limit,
      }),
    );
    if (nextHistoryError != null) {
      final err = nextHistoryError;
      nextHistoryError = null;
      throw err!;
    }
    if (historyPages.isNotEmpty) {
      return historyPages.removeAt(0);
    }
    return const HistoryPage(entries: [], nextCursor: null);
  }

  @override
  Future<Stats> getStats() async {
    calls.add(FakeCall('GET', '/api/v1/stats'));
    if (nextStatsError != null) {
      final err = nextStatsError;
      nextStatsError = null;
      throw err!;
    }
    return stats;
  }

  /// Returned by [getMyExperiments]; [putMyExperiments] updates it.
  MyExperiments myExperiments = const MyExperiments(
    available: true,
    optedIn: false,
    consentVersion: null,
    currentConsentVersion: '2026-10-03',
    optedInAt: null,
  );

  /// When set, the matching experiments call throws it (and clears it).
  Object? nextExperimentsGetError;
  Object? nextExperimentsPutError;

  /// Admin lists. Pages are consumed in order; when empty a single page of
  /// the matching `*Items` list is returned.
  final List<Paged<Invite>> invitePages = [];
  final List<Paged<InviteRequest>> inviteRequestPages = [];
  final List<Paged<AdminUser>> userPages = [];

  /// Throws (and clears) on the next admin call of any kind.
  Object? nextAdminError;

  /// Throws (and clears) on the next admin write, so a test can make the
  /// first attempt ask for step-up and the retry succeed.
  Object? nextAdminWriteError;

  Future<void> _adminWrite(FakeCall call) async {
    calls.add(call);
    final err = nextAdminWriteError;
    if (err != null) {
      nextAdminWriteError = null;
      throw err;
    }
  }

  void _adminRead(FakeCall call) {
    calls.add(call);
    final err = nextAdminError;
    if (err != null) {
      nextAdminError = null;
      throw err;
    }
  }

  Invite _invite(String id, String email) => Invite(
    inviteId: id,
    emailAddress: email,
    status: InviteStatus.pending,
    createdAt: DateTime.utc(2026, 10),
    expiresAt: DateTime.utc(2026, 10, 8),
    lastSentAt: DateTime.utc(2026, 10),
  );

  @override
  Future<MyExperiments> getMyExperiments() async {
    calls.add(FakeCall('GET', '/api/v1/me/experiments'));
    if (nextExperimentsGetError != null) {
      final err = nextExperimentsGetError;
      nextExperimentsGetError = null;
      throw err!;
    }
    return myExperiments;
  }

  @override
  Future<MyExperiments> putMyExperiments({
    required bool optedIn,
    required String consentVersion,
  }) async {
    calls.add(
      FakeCall('PUT', '/api/v1/me/experiments', {
        'opted_in': optedIn,
        'consent_version': consentVersion,
      }),
    );
    if (nextExperimentsPutError != null) {
      final err = nextExperimentsPutError;
      nextExperimentsPutError = null;
      throw err!;
    }
    myExperiments = MyExperiments(
      available: myExperiments.available,
      optedIn: optedIn,
      consentVersion: optedIn ? consentVersion : null,
      currentConsentVersion: myExperiments.currentConsentVersion,
      optedInAt: optedIn ? DateTime.utc(2026, 10, 5) : null,
    );
    return myExperiments;
  }

  @override
  Future<Paged<Invite>> listInvites({String? cursor}) async {
    _adminRead(FakeCall('GET', '/api/v1/admin/invites', {'cursor': cursor}));
    return invitePages.isEmpty
        ? const Paged<Invite>(items: [])
        : invitePages.removeAt(0);
  }

  @override
  Future<Invite> createInvite(String emailAddress) async {
    await _adminWrite(
      FakeCall('POST', '/api/v1/admin/invites', {
        'email_address': emailAddress,
      }),
    );
    return _invite('inv-new', emailAddress);
  }

  @override
  Future<Invite> resendInvite(String inviteId) async {
    await _adminWrite(
      FakeCall('POST', '/api/v1/admin/invites/$inviteId/resend'),
    );
    return _invite(inviteId, 'resent@example.test');
  }

  @override
  Future<void> revokeInvite(String inviteId) =>
      _adminWrite(FakeCall('DELETE', '/api/v1/admin/invites/$inviteId'));

  @override
  Future<Paged<InviteRequest>> listInviteRequests({String? cursor}) async {
    _adminRead(
      FakeCall('GET', '/api/v1/admin/invite-requests', {'cursor': cursor}),
    );
    return inviteRequestPages.isEmpty
        ? const Paged<InviteRequest>(items: [])
        : inviteRequestPages.removeAt(0);
  }

  @override
  Future<Invite> approveInviteRequest(String requestId) async {
    await _adminWrite(
      FakeCall('POST', '/api/v1/admin/invite-requests/$requestId/approve'),
    );
    return _invite('inv-approved', 'approved@example.test');
  }

  @override
  Future<void> declineInviteRequest(String requestId) => _adminWrite(
    FakeCall('POST', '/api/v1/admin/invite-requests/$requestId/decline'),
  );

  @override
  Future<Paged<AdminUser>> listUsers({String? cursor}) async {
    _adminRead(FakeCall('GET', '/api/v1/admin/users', {'cursor': cursor}));
    return userPages.isEmpty
        ? const Paged<AdminUser>(items: [])
        : userPages.removeAt(0);
  }

  @override
  Future<void> endUserSession(String userId) =>
      _adminWrite(FakeCall('DELETE', '/api/v1/admin/users/$userId/sessions'));
}
