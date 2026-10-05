import 'dart:async';

import 'package:flutter/foundation.dart';

import '../api/api_client.dart';
import '../api/models/feed.dart';
import '../api/models/session.dart';
import '../platform/connectivity.dart';
import 'session_model.dart';

enum FeedStatus { loading, ready, empty, allNeedSignIn, loadFailed }

sealed class FeedItem {}

final class CardItem extends FeedItem {
  CardItem(this.card);

  final FeedCard card;
}

final class DividerItem extends FeedItem {}

/// Drives the Feed tab: loads pages of cards from API-FEED-1 and exposes the
/// queue, the current/next items and every non-swipe Feed state.
///
/// The queue lives here only; no card, preview or cursor is written to browser
/// storage. Swipe operations are added in T-1002b.
class FeedModel extends ChangeNotifier {
  FeedModel({
    required ApiClient api,
    required SessionModel session,
    required Connectivity connectivity,
    int pageSize = 20,
    int refillBelow = 5,
  }) : _api = api,
       _session = session,
       _connectivity = connectivity,
       _pageSize = pageSize,
       _refillBelow = refillBelow {
    _connectivitySub = _connectivity.changes.listen(_onConnectivityChanged);
    _session.addWipeListener(_onWipe);
  }

  final ApiClient _api;
  final SessionModel _session;
  final Connectivity _connectivity;
  final int _pageSize;
  final int _refillBelow;

  final List<FeedItem> _queue = <FeedItem>[];
  StreamSubscription<bool>? _connectivitySub;

  FeedStatus _status = FeedStatus.loading;
  String? _nextCursor;
  List<MailboxError> _mailboxErrors = const <MailboxError>[];
  bool _offline = false;
  bool _loadingInFlight = false;
  int _emptyPageStreak = 0;
  bool _dividerShown = false;

  FeedStatus get status => _status;
  FeedItem? get current => _queue.isEmpty ? null : _queue.first;
  FeedItem? get next => _queue.length > 1 ? _queue[1] : null;
  List<MailboxError> get mailboxErrors => _mailboxErrors;
  bool get offline => _offline;

  /// First open of the Feed tab: cursor null, refresh true.
  Future<void> open() => _loadInitial();

  /// Pull to refresh: clears the queue, same call as [open].
  Future<void> refresh() => _loadInitial();

  /// Fetches the next page when fewer than [refillBelow] items remain and a
  /// cursor exists. Stops once three empty pages in a row have been fetched
  /// (rules may trash a whole page, but not more than that).
  Future<void> loadMoreIfNeeded() async {
    if (_loadingInFlight) return;
    if (_queue.length >= _refillBelow ||
        _nextCursor == null ||
        _emptyPageStreak >= 3) {
      return;
    }

    _loadingInFlight = true;
    try {
      await _request(cursor: _nextCursor, refresh: false);
    } on NetworkException {
      _offline = true;
      notifyListeners();
    } on Object {
      if (_queue.isEmpty) {
        _status = FeedStatus.loadFailed;
      }
      notifyListeners();
    } finally {
      _loadingInFlight = false;
    }
    if (_queue.isEmpty && _nextCursor != null && _emptyPageStreak < 3) {
      await _cascadeEmpty();
    }
  }

  /// Dismisses the up-to-date divider card.
  void dismissDivider() {
    final first = _queue.isEmpty ? null : _queue.first;
    if (first is DividerItem) {
      _queue.removeAt(0);
      notifyListeners();
      loadMoreIfNeeded();
    }
  }

  /// Removes and returns the current card (null for a divider).
  FeedCard? takeCurrent() {
    if (_queue.isEmpty) return null;
    final first = _queue.first;
    if (first is! CardItem) return null;
    _queue.removeAt(0);
    notifyListeners();
    loadMoreIfNeeded();
    return first.card;
  }

  /// Puts [card] back on top of the queue (undo).
  void putBackOnTop(FeedCard card) {
    _queue.insert(0, CardItem(card));
    notifyListeners();
  }

  /// Drops a card for a message handled elsewhere (FD-04).
  void dropMessage(String mailboxId, String messageId) {
    _queue.removeWhere(
      (item) =>
          item is CardItem &&
          item.card.mailboxId == mailboxId &&
          item.card.messageId == messageId,
    );
    notifyListeners();
  }

  Future<void> _loadInitial() async {
    if (_loadingInFlight) return;
    _loadingInFlight = true;
    _queue.clear();
    _nextCursor = null;
    _emptyPageStreak = 0;
    _dividerShown = false;
    _status = FeedStatus.loading;
    _offline = false;
    notifyListeners();
    try {
      await _request(cursor: null, refresh: true);
      await _cascadeEmpty();
    } on NetworkException {
      _offline = true;
      notifyListeners();
    } on Object {
      if (_queue.isEmpty) {
        _status = FeedStatus.loadFailed;
      }
      notifyListeners();
    } finally {
      _loadingInFlight = false;
    }
    await loadMoreIfNeeded();
  }

  /// Keeps the queue topped up across empty pages (rules may trash a whole
  /// page), at most three empty pages in a row.
  Future<void> _cascadeEmpty() async {
    while (_queue.isEmpty && _nextCursor != null && _emptyPageStreak < 3) {
      try {
        await _request(cursor: _nextCursor, refresh: false);
      } on NetworkException {
        _offline = true;
        notifyListeners();
        return;
      } on Object {
        if (_queue.isEmpty) {
          _status = FeedStatus.loadFailed;
        }
        notifyListeners();
        return;
      }
    }
  }

  /// One API-FEED-1 request, applied. Callers manage `_loadingInFlight`.
  /// Throws on failure so the orchestration above can set status.
  Future<void> _request({
    required String? cursor,
    required bool refresh,
  }) async {
    final page = await _api.feedNext(
      cursor: cursor,
      limit: _pageSize,
      refresh: refresh,
    );
    _nextCursor = page.nextCursor;
    _mailboxErrors = List<MailboxError>.unmodifiable(page.mailboxErrors);
    if (page.phaseChanged && !_dividerShown) {
      _queue.add(DividerItem());
      _dividerShown = true;
    }
    _queue.addAll(page.cards.map(CardItem.new));
    if (page.cards.isEmpty) {
      _emptyPageStreak++;
    } else {
      _emptyPageStreak = 0;
    }
    _updateStatus();
    notifyListeners();
  }

  void _updateStatus() {
    if (_queue.isNotEmpty) {
      _status = FeedStatus.ready;
      return;
    }
    final boxes = _session.session?.mailboxes ?? const <Mailbox>[];
    final needsSignIn =
        boxes.isNotEmpty &&
        boxes.every(
          (b) => _mailboxErrors.any(
            (e) =>
                e.mailboxId == b.mailboxId && e.code == 'mailbox_needs_sign_in',
          ),
        );
    _status = needsSignIn ? FeedStatus.allNeedSignIn : FeedStatus.empty;
  }

  void _onConnectivityChanged(bool online) {
    if (!online) return;
    _offline = false;
    if (_queue.isEmpty) {
      unawaited(open());
    } else {
      notifyListeners();
    }
  }

  void _onWipe() {
    _queue.clear();
    _nextCursor = null;
    _status = FeedStatus.loading;
    _mailboxErrors = const <MailboxError>[];
    _offline = false;
    notifyListeners();
  }

  @override
  void dispose() {
    _connectivitySub?.cancel();
    _session.removeWipeListener(_onWipe);
    super.dispose();
  }
}
