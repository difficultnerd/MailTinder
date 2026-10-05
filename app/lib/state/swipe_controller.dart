import 'dart:async';

import 'package:flutter/material.dart';

import '../api/api_client.dart';
import '../api/models/feed.dart';
import '../api/models/swipe.dart';
import '../copy.dart';
import '../screens/feed/block_prompt.dart';
import 'feed_model.dart';
import 'id_generator.dart';

/// A filing choice from the filing sheet (T-1003 supplies the real launcher).
class FilingChoice {
  const FilingChoice({this.categoryId, this.newCategoryName});

  final String? categoryId;
  final String? newCategoryName;
}

/// Opens the filing sheet for [card] and returns the choice, or null on
/// cancel. The default launcher in this task returns null; T-1003 replaces it.
typedef FilingSheetLauncher =
    Future<FilingChoice?> Function(BuildContext context, FeedCard card);

/// Events emitted by [SwipeController.events] for later tasks (T-1008a
/// celebrations, T-1009 Blitz) to listen to.
sealed class SwipeEvent {}

final class SwipeAcked extends SwipeEvent {
  SwipeAcked(this.card, this.kind, this.result);

  final FeedCard card;
  final SwipeKind kind;
  final SwipeResult result;
}

final class SwipeUndone extends SwipeEvent {
  SwipeUndone(this.card, this.kind, this.result);

  final FeedCard card;
  final SwipeKind kind;
  final SwipeResult result;
}

final class BlockAccepted extends SwipeEvent {
  BlockAccepted(this.senderName);

  final String senderName;
}

/// A toast to show after a swipe or undo.
class ToastMessage {
  const ToastMessage({required this.text, required this.showUndo});

  final String text;
  final bool showUndo;
}

/// One entry on the in-memory undo stack (S3 `UndoStack`). Nothing is stored
/// in browser storage; the stack is empty in a new app instance.
class UndoEntry {
  UndoEntry({
    required this.card,
    required this.kind,
    required this.key,
    required this.ack,
  });

  final FeedCard card;
  final SwipeKind kind;
  final String key;
  final Completer<SwipeResult> ack;
  SwipeResult? result;
}

/// UNSUB_DELAY [TUNABLE], S2 glossary.
const Duration kUnsubDelay = Duration(minutes: 5);

/// PERSONAL_BLOCK_THRESHOLD [TUNABLE], S2 glossary.
const int kPersonalBlockThreshold = 3;

/// Drives swipes, undo and the block prompt on the Feed (S7 5.5, S9 section 3).
///
/// Swipes are optimistic: the card leaves and a toast shows at once, then
/// API-SW-1 is sent in the background, one at a time in order. Undo waits for
/// its swipe's ack before sending API-SW-2. Failures put the card back with
/// `Copy.actionFailed`; `409 message_changed` drops the card silently (FD-04).
class SwipeController extends ChangeNotifier {
  SwipeController({
    required ApiClient api,
    required FeedModel feed,
    required IdGenerator ids,
    DateTime Function()? now,
    FilingSheetLauncher? fileLauncher,
    BuildContext Function()? contextProvider,
    Duration retryBase = const Duration(seconds: 1),
  }) : _api = api,
       _feed = feed,
       _ids = ids,
       _now = now ?? DateTime.now,
       _fileLauncher = fileLauncher ?? _defaultFileLauncher,
       _contextProvider = contextProvider,
       _retryBase = retryBase;

  final ApiClient _api;
  final FeedModel _feed;
  final IdGenerator _ids;
  final DateTime Function() _now;
  final FilingSheetLauncher _fileLauncher;
  final BuildContext Function()? _contextProvider;
  final Duration _retryBase;

  final List<UndoEntry> _undoStack = [];
  final StreamController<SwipeEvent> _events =
      StreamController<SwipeEvent>.broadcast();

  Future<void> _tail = Future<void>.value();
  ToastMessage? _toast;
  final List<BlockPrompt> _heldPrompts = [];

  /// Whether the undo stack has at least one entry.
  bool get canUndo => _undoStack.isNotEmpty;

  /// False while offline or when there is nothing to act on. A divider counts:
  /// any swipe or button dismisses it (and sends nothing).
  bool get enabled => !_feed.offline && _feed.current != null;

  /// The current toast, or null when none is showing.
  ToastMessage? get toast => _toast;

  /// When true, block prompts are held (T-1009 sets this during a Blitz round).
  bool holdPrompts = false;

  /// Returns and clears any held block prompts.
  List<BlockPrompt> takeHeldPrompts() {
    final held = List<BlockPrompt>.from(_heldPrompts);
    _heldPrompts.clear();
    return held;
  }

  Stream<SwipeEvent> get events => _events.stream;

  Future<void> keep() => _swipe(SwipeKind.keep);
  Future<void> skip() => _swipe(SwipeKind.skip);
  Future<void> reject() => _swipe(SwipeKind.reject);

  Future<void> file(BuildContext context) async {
    if (!enabled) return;
    final current = _feed.current;
    if (current is! CardItem) {
      if (current is DividerItem) {
        _feed.dismissDivider();
      }
      return;
    }
    final card = current.card;
    final choice = await _fileLauncher(context, card);
    if (choice == null) {
      return; // cancelled: card stays, nothing sent
    }
    await _swipe(
      SwipeKind.file,
      card: card,
      categoryId: choice.categoryId,
      newCategoryName: choice.newCategoryName,
    );
  }

  Future<void> undo() async {
    if (_undoStack.isEmpty) return;
    final entry = _undoStack.removeLast();
    _notify();
    // Wait for the swipe's ack (or its failure) before sending API-SW-2.
    _tail = _tail.then((_) => _sendUndo(entry));
    await _tail;
  }

  Future<void> _swipe(
    SwipeKind kind, {
    FeedCard? card,
    String? categoryId,
    String? newCategoryName,
  }) async {
    if (!enabled) return;
    final current = _feed.current;
    if (current is! CardItem) {
      if (current is DividerItem) {
        _feed.dismissDivider();
      }
      return;
    }
    // Remove the card from the feed optimistically (S7 5.5). For a file the
    // card was already captured for the filing sheet; takeCurrent returns it.
    final target = _feed.takeCurrent() ?? (card ?? current.card);
    final key = _ids.uuidV4();
    final ack = Completer<SwipeResult>();
    final entry = UndoEntry(card: target, kind: kind, key: key, ack: ack);
    _undoStack.add(entry);
    _toast = _optimisticToast(target, kind);
    _notify();
    _tail = _tail.then((_) => _send(entry, categoryId, newCategoryName));
    await _tail;
  }

  Future<void> _send(
    UndoEntry entry,
    String? categoryId,
    String? newCategoryName,
  ) async {
    final req = SwipeRequest(
      mailboxId: entry.card.mailboxId,
      messageId: entry.card.messageId,
      classificationToken: entry.card.classificationToken,
      kind: entry.kind,
      categoryId: categoryId,
      newCategoryName: newCategoryName,
    );

    SwipeResult result;
    try {
      result = await _sendWithRetry(req, entry.key);
    } on ApiException catch (e) {
      if (e.code == 'message_changed') {
        // FD-04 AC1: card stays gone, silently.
        _undoStack.remove(entry);
        if (_toast != null && _toast!.showUndo) {
          _toast = null;
        }
        _notify();
        return;
      }
      _fail(entry);
      return;
    } on Object {
      _fail(entry);
      return;
    }

    entry.result = result;
    if (!entry.ack.isCompleted) {
      entry.ack.complete(result);
    }
    _events.add(SwipeAcked(entry.card, entry.kind, result));
    if (_toast != null && _toast!.showUndo) {
      _toast = ToastMessage(text: _outcomeToast(result), showUndo: true);
    }
    _notify();
    await _handlePrompts(result.prompts);
  }

  Future<SwipeResult> _sendWithRetry(SwipeRequest req, String key) async {
    var attempt = 0;
    while (true) {
      try {
        return await _api.swipe(req, idempotencyKey: key);
      } on NetworkException {
        attempt++;
        if (attempt > 3) {
          rethrow;
        }
        await Future<void>.delayed(_retryBase * (1 << (attempt - 1)));
      }
    }
  }

  void _fail(UndoEntry entry) {
    _undoStack.remove(entry);
    // The send chain is sequential, so an undo queued behind this swipe runs
    // after it and sees `result == null`; the ack is never completed on
    // failure (completing it with an error would surface as an unhandled
    // async error when nothing is waiting on it).
    _feed.putBackOnTop(entry.card);
    _toast = const ToastMessage(text: Copy.actionFailed, showUndo: false);
    _notify();
  }

  Future<void> _sendUndo(UndoEntry entry) async {
    final result = entry.result;
    if (result == null) {
      // The swipe failed (card already back); nothing to undo.
      return;
    }
    try {
      final undoResult = await _api.undo(result.undoToken);
      _feed.putBackOnTop(entry.card);
      _events.add(SwipeUndone(entry.card, entry.kind, result));
      _toast = ToastMessage(
        text: _undoToast(undoResult, result.outcome),
        showUndo: false,
      );
      _notify();
    } on ApiException catch (e) {
      if (e.code == 'undo_expired') {
        _undoStack.clear();
        _toast = const ToastMessage(text: Copy.cannotUndo, showUndo: false);
        _notify();
        return;
      }
      _undoStack.add(entry);
      _toast = const ToastMessage(text: Copy.actionFailed, showUndo: false);
      _notify();
    } on Object {
      _undoStack.add(entry);
      _toast = const ToastMessage(text: Copy.actionFailed, showUndo: false);
      _notify();
    }
  }

  Future<void> _handlePrompts(List<BlockPrompt> prompts) async {
    for (final prompt in prompts) {
      if (holdPrompts) {
        _heldPrompts.add(prompt);
        continue;
      }
      final blocked = await _showBlockPrompt(prompt);
      if (blocked == null) return; // dialog dismissed
      if (blocked) {
        try {
          await _api.createBlockRule(prompt.promptRef);
          _events.add(BlockAccepted(prompt.senderName));
          _toast = const ToastMessage(text: Copy.blocked, showUndo: false);
          _notify();
        } on Object {
          _toast = const ToastMessage(text: Copy.actionFailed, showUndo: false);
          _notify();
        }
      } else {
        try {
          await _api.declineBlockPrompt(prompt.promptRef);
        } on Object {
          _toast = const ToastMessage(text: Copy.actionFailed, showUndo: false);
          _notify();
        }
      }
    }
  }

  /// Shows the block dialog (PB-01). Returns true for Block, false for Not
  /// now, null when dismissed.
  Future<bool?> _showBlockPrompt(BlockPrompt prompt) async {
    final context = _contextProvider?.call();
    if (context == null) return null;
    return showDialog<bool>(
      context: context,
      builder: (context) => BlockPromptDialog(senderName: prompt.senderName),
    );
  }

  ToastMessage _optimisticToast(FeedCard card, SwipeKind kind) {
    final text = switch (kind) {
      SwipeKind.keep => Copy.kept,
      SwipeKind.skip => Copy.skipped,
      SwipeKind.reject =>
        card.messageClass == MessageClass.list && card.hasOneClick
            ? Copy.trashedUnsubscribing(kUnsubDelay)
            : Copy.trashed,
      SwipeKind.file => Copy.filed(''),
    };
    return ToastMessage(text: text, showUndo: true);
  }

  String _outcomeToast(SwipeResult result) {
    final outcome = result.outcome;
    return switch (outcome) {
      'kept' => Copy.kept,
      'skipped' => Copy.skipped,
      'trashed_unsubscribe_queued' => Copy.trashedUnsubscribing(
        _unsubDelay(result.unsubscribeDueAt),
      ),
      'trashed_unsubscribe_manual' => Copy.trashedUnsubscribeManual,
      'trashed_list_no_unsubscribe' => Copy.trashedListNoUnsubscribe,
      'trashed' => Copy.trashed,
      'reported_spam' => Copy.reportedSpam,
      'filed' => Copy.filed(result.filedCategory?.name ?? ''),
      _ => Copy.trashed,
    };
  }

  Duration _unsubDelay(DateTime? dueAt) {
    if (dueAt == null) return kUnsubDelay;
    final diff = dueAt.difference(_now());
    if (diff <= Duration.zero) return const Duration(minutes: 1);
    return diff;
  }

  String _undoToast(UndoResult undoResult, String outcome) {
    if (undoResult.unsubscribeAlreadySent) return Copy.restoredAlreadySent;
    if (outcome == 'reported_spam') return Copy.restoredSpam;
    return Copy.restored;
  }

  void _notify() {
    notifyListeners();
  }

  static Future<FilingChoice?> _defaultFileLauncher(
    BuildContext context,
    FeedCard card,
  ) async {
    return null;
  }

  @override
  void dispose() {
    _events.close();
    super.dispose();
  }
}
