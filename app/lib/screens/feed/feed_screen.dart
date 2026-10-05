import 'dart:async';

import 'package:flutter/foundation.dart' show ValueListenable;
import 'package:flutter/material.dart';

import '../../api/api_client.dart';
import '../../api/models/auth.dart';
import '../../api/models/feed.dart';
import '../../api/models/session.dart';
import '../../api/models/swipe.dart';
import '../../copy.dart';
import '../../platform/browser.dart';
import '../../platform/haptics.dart';
import '../../platform/sound_player.dart';
import '../../state/categories_cache.dart';
import '../../state/feed_model.dart';
import '../../state/feedback_model.dart';
import '../../state/id_generator.dart';
import '../../state/play_prefs.dart';
import '../../state/progress_model.dart';
import '../../state/round_tracker.dart';
import '../../state/session_model.dart';
import '../../state/swipe_controller.dart';
import '../settings/app_scope.dart';
import '../../state/blitz_model.dart';
import 'blitz_bar.dart';
import 'blitz_results_card.dart';
import 'card_view.dart';
import 'celebrations.dart';
import 'effects.dart';
import 'divider_card.dart';
import 'filing_sheet.dart';
import 'progress_header.dart';
import 'round_card.dart';
import 'swipe_buttons.dart';
import 'swipeable_card.dart';

/// The Feed tab (S9 section 3): one card in focus with the next behind it,
/// swipe gestures and buttons, the toast with Undo, and every non-swipe Feed
/// state.
///
/// [model] powers the state. [session], [api] and [browser] let the banner and
/// full-screen "Sign in again" actions start a reconnect. When [model] is
/// null the screen renders the placeholder (the HomeShell builds it without
/// dependencies until a later task wires the live model).
class FeedScreen extends StatefulWidget {
  const FeedScreen({
    super.key,
    this.model,
    this.session,
    this.api,
    this.browser,
    this.categories,
    this.progress,
    this.rounds,
    this.feedVisible,
    this.feedback,
  });

  final FeedModel? model;
  final SessionModel? session;
  final ApiClient? api;
  final Browser? browser;

  /// The session's category cache; injected in tests. Built from [api] when
  /// absent.
  final CategoriesCache? categories;

  /// The meter model; built from [api] when absent.
  final ProgressModel? progress;

  /// The round totals; a fresh tracker when absent.
  final RoundTracker? rounds;

  /// Whether the Feed tab is selected. When it turns false the round card
  /// becomes due for the next time the Feed shows (GM-03 AC1).
  final ValueListenable<bool>? feedVisible;

  /// Swipe effects (GM-02); built from the app scope's Sounds switch when
  /// absent.
  final FeedbackModel? feedback;

  @override
  State<FeedScreen> createState() => _FeedScreenState();
}

class _FeedScreenState extends State<FeedScreen> {
  SwipeController? _swipe;
  ProgressModel? _progress;
  RoundTracker? _rounds;
  FeedbackModel? _feedback;
  BlitzModel? _blitz;
  final GlobalKey<SwipeableCardState> _cardKey = GlobalKey();
  final CelebrationQueue _celebrations = CelebrationQueue();
  StreamSubscription<SwipeEvent>? _eventsSub;
  FeedItem? _dividerMarked;
  bool _visible = true;

  @override
  void initState() {
    super.initState();
    final model = widget.model;
    if (model != null) {
      model.open();
      final progress = _progress =
          widget.progress ?? ProgressModel(api: widget.api!);
      progress.addListener(_onProgressChanged);
      unawaited(progress.load());
      _rounds = widget.rounds ?? RoundTracker();
      _feedback =
          widget.feedback ??
          FeedbackModel(
            prefs:
                context.getInheritedWidgetOfExactType<AppScope>()?.playPrefs ??
                PlayPrefs(),
            sound: const SoundPlayerImpl(),
            haptics: const HapticsImpl(),
          );
      model.addListener(_onFeedChanged);
      final visible = widget.feedVisible;
      if (visible != null) {
        _visible = visible.value;
        visible.addListener(_onVisibleChanged);
      }
      final categories = widget.categories ?? CategoriesCache(api: widget.api!);
      // Load the category names in the background so the first up-swipe has
      // them (FL-01 AC3).
      unawaited(categories.ensureLoaded());
      _swipe = SwipeController(
        api: widget.api!,
        feed: model,
        ids: IdGenerator(),
        contextProvider: () => context,
        categories: categories,
        fileLauncher: (context, card) =>
            showFilingSheet(context, card, cache: categories),
      )..addListener(_onSwipeChanged);
      _eventsSub = _swipe!.events.listen(_onSwipeEvent);
      _blitz = BlitzModel(feed: model, swipes: _swipe!);
    }
  }

  void _onSwipeEvent(SwipeEvent e) {
    _rounds?.onEvent(e);
    _celebrations.onEvent(e);
    if (e is SwipeAcked) _progress?.onSwipeAcked();
    // The swipe that revealed the divider was optimistic, so its ack (and the
    // swipe count) arrives after the divider became current.
    if (widget.model?.current is DividerItem) _rounds?.markDividerReached();
  }

  void _onProgressChanged() {
    final done = _progress?.takeCompletedLevel();
    final now = _progress?.level;
    if (done != null && now != null) {
      _celebrations.addLevelComplete(done.year, now.year);
    }
  }

  void _onFeedChanged() {
    final current = widget.model?.current;
    if (current is DividerItem && !identical(current, _dividerMarked)) {
      _dividerMarked = current;
      _rounds?.markDividerReached();
    }
  }

  void _onVisibleChanged() {
    final visible = widget.feedVisible!.value;
    if (_visible && !visible) _rounds?.markLeftFeed();
    setState(() => _visible = visible);
  }

  @override
  void dispose() {
    widget.model?.removeListener(_onFeedChanged);
    widget.feedVisible?.removeListener(_onVisibleChanged);
    _progress?.removeListener(_onProgressChanged);
    if (widget.progress == null) _progress?.dispose();
    if (widget.rounds == null) _rounds?.dispose();
    if (widget.feedback == null) _feedback?.dispose();
    _eventsSub?.cancel();
    _blitz?.dispose();
    _celebrations.dispose();
    _swipe?.removeListener(_onSwipeChanged);
    _swipe?.dispose();
    super.dispose();
  }

  void _onSwipeChanged() {
    final toast = _swipe?.toast;
    final messenger = ScaffoldMessenger.of(context);
    if (toast == null) {
      // e.g. a `409 message_changed` drops the card silently (FD-04).
      messenger.removeCurrentSnackBar();
      return;
    }
    messenger.removeCurrentSnackBar();
    messenger.showSnackBar(
      SnackBar(
        content: Text(toast.text),
        duration: const Duration(seconds: 4),
        action: toast.showUndo
            ? SnackBarAction(label: Copy.undoButton, onPressed: _swipe!.undo)
            : null,
      ),
    );
  }

  String _mailboxAddress(SessionModel session, String mailboxId) {
    for (final m in session.session?.mailboxes ?? const <Mailbox>[]) {
      if (m.mailboxId == mailboxId) return m.emailAddress;
    }
    return '';
  }

  Future<void> _reconnect(
    ApiClient api,
    Browser browser,
    String mailboxId,
  ) async {
    try {
      final url = await api.startAuth(
        intent: AuthIntent.reconnect,
        mailboxId: mailboxId,
      );
      final target = safeNavigationTarget(
        url.toString(),
        allowLoopbackHttp: false,
      );
      if (target != null) browser.assign(target);
    } on Object {
      // Leave the banner in place; the page does not navigate.
    }
  }

  @override
  Widget build(BuildContext context) {
    final model = widget.model;
    if (model == null) {
      return const Center(child: Text(Copy.tabFeed));
    }
    final session = widget.session!;
    final api = widget.api!;
    final browser = widget.browser!;

    return Stack(
      children: [
        ListenableBuilder(
          listenable: model,
          builder: (context, _) {
            final current = model.current;
            return Column(
              children: [
                if (model.offline) const _OfflineBanner(),
                for (final err in model.mailboxErrors)
                  ..._mailboxBanners(err, session, api, browser),
                ProgressHeader(
                  progress: _progress!,
                  current: current is CardItem ? current.card : null,
                ),
                BlitzBar(model: _blitz!),
                Expanded(
                  child: _buildBody(context, model, session, api, browser),
                ),
              ],
            );
          },
        ),
        CelebrationOverlay(queue: _celebrations),
        ListenableBuilder(
          listenable: _feedback!,
          builder: (context, _) {
            final fb = _feedback!;
            return Stack(
              children: [
                if (fb.combo != null)
                  Positioned(
                    top: 8,
                    right: 8,
                    child: ComboBadge(count: fb.combo!),
                  ),
                if (fb.confettiDue)
                  Positioned.fill(
                    child: MilestoneOverlay(
                      key: ValueKey('milestone-${fb.milestone}'),
                      milestone: fb.milestone,
                      onDone: fb.acknowledgeConfetti,
                    ),
                  ),
              ],
            );
          },
        ),
        ListenableBuilder(
          listenable: _blitz!,
          builder: (context, _) {
            final result = _blitz!.result;
            if (_blitz!.status != BlitzStatus.ended || result == null) {
              return const SizedBox.shrink();
            }
            return Positioned.fill(
              child: BlitzResultsCard(
                result: result,
                onDone: _blitz!.dismissResults,
              ),
            );
          },
        ),
        ListenableBuilder(
          listenable: _rounds!,
          builder: (context, _) {
            if (!_visible || !_rounds!.cardDue) {
              return const SizedBox.shrink();
            }
            return Positioned.fill(
              child: RoundCard(
                totals: _rounds!.totals,
                onDismiss: _rounds!.takeCard,
              ),
            );
          },
        ),
      ],
    );
  }

  List<Widget> _mailboxBanners(
    MailboxError err,
    SessionModel session,
    ApiClient api,
    Browser browser,
  ) {
    final address = _mailboxAddress(session, err.mailboxId);
    if (err.code == 'mailbox_needs_sign_in') {
      return [
        _MailboxBanner(
          text: Copy.mailboxNeedsSignIn(address),
          action: Semantics(
            label: Copy.signInAgain,
            button: true,
            child: TextButton(
              onPressed: () => _reconnect(api, browser, err.mailboxId),
              child: const Text(Copy.signInAgain),
            ),
          ),
        ),
      ];
    }
    return [_MailboxBanner(text: Copy.mailboxUnavailable(address))];
  }

  Widget _buildBody(
    BuildContext context,
    FeedModel m,
    SessionModel session,
    ApiClient api,
    Browser browser,
  ) {
    // Offline with nothing loaded yet: show the Feed layout so the offline
    // banner and the (disabled) swipe buttons are both on screen (S9).
    if (m.offline && m.status == FeedStatus.loading) {
      return _buildReady(context, m, session);
    }
    return switch (m.status) {
      FeedStatus.loading => Center(
        child: Semantics(
          label: Copy.loadingCards,
          child: const CircularProgressIndicator(),
        ),
      ),
      FeedStatus.ready => _buildReady(context, m, session),
      FeedStatus.empty => const Center(child: Text(Copy.nothingToTriage)),
      FeedStatus.allNeedSignIn => _buildAllNeedSignIn(session, api, browser),
      FeedStatus.loadFailed => _buildLoadFailed(m),
    };
  }

  /// No card widget to animate (e.g. the divider): act directly.
  void _direct(SwipeController swipe, SwipeKind kind) {
    switch (kind) {
      case SwipeKind.keep:
        swipe.keep();
      case SwipeKind.reject:
        swipe.reject();
      case SwipeKind.file:
        swipe.file(context);
      case SwipeKind.skip:
        swipe.skip();
    }
  }

  Widget _buildReady(BuildContext context, FeedModel m, SessionModel session) {
    final current = m.current;
    final next = m.next;
    final swipe = _swipe!;
    return LayoutBuilder(
      builder: (context, constraints) {
        return RefreshIndicator(
          onRefresh: m.refresh,
          child: SingleChildScrollView(
            physics: const AlwaysScrollableScrollPhysics(),
            child: SizedBox(
              height: constraints.maxHeight,
              child: Padding(
                padding: const EdgeInsets.fromLTRB(16, 16, 16, 32),
                child: Column(
                  children: [
                    Expanded(
                      child: Stack(
                        children: [
                          if (next is CardItem)
                            Positioned.fill(
                              child: _backgroundCard(next.card, session),
                            ),
                          Positioned.fill(
                            child: _currentCard(
                              context,
                              current,
                              session,
                              swipe,
                            ),
                          ),
                        ],
                      ),
                    ),
                    const SizedBox(height: 16),
                    SwipeButtons(
                      controller: swipe,
                      onSwipe: (kind) {
                        final card = _cardKey.currentState;
                        if (card != null) {
                          card.swipe(kind);
                        } else {
                          _direct(swipe, kind);
                        }
                      },
                    ),
                  ],
                ),
              ),
            ),
          ),
        );
      },
    );
  }

  /// The card behind the focused one: offset 12px down and scaled 0.95,
  /// excluded from semantics so screen readers hear one card.
  Widget _backgroundCard(FeedCard card, SessionModel session) {
    return ExcludeSemantics(
      child: Transform.translate(
        offset: const Offset(0, 12),
        child: Transform.scale(
          scale: 0.95,
          child: Align(
            alignment: Alignment.topLeft,
            child: SizedBox(
              width: double.infinity,
              child: CardView(
                card: card,
                mailboxAddress: _mailboxAddress(session, card.mailboxId),
              ),
            ),
          ),
        ),
      ),
    );
  }

  Widget _currentCard(
    BuildContext context,
    FeedItem? current,
    SessionModel session,
    SwipeController swipe,
  ) {
    if (current is CardItem) {
      return SwipeableCard(
        key: _cardKey,
        onKeep: swipe.keep,
        onReject: swipe.reject,
        onFile: () => swipe.file(context),
        onSkip: swipe.skip,
        feedback: _feedback,
        card: current.card,
        child: CardView(
          card: current.card,
          mailboxAddress: _mailboxAddress(session, current.card.mailboxId),
          onFilePrompt: () => swipe.acceptKeepPrompt(current.card),
        ),
      );
    }
    if (current is DividerItem) {
      return Align(
        alignment: Alignment.topCenter,
        child: DividerCard(onContinue: widget.model!.dismissDivider),
      );
    }
    return const SizedBox.shrink();
  }

  Widget _buildAllNeedSignIn(
    SessionModel session,
    ApiClient api,
    Browser browser,
  ) {
    return Center(
      child: Padding(
        padding: const EdgeInsets.all(24),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            const Text(Copy.allNeedSignIn, textAlign: TextAlign.center),
            const SizedBox(height: 16),
            for (final box in session.session?.mailboxes ?? const <Mailbox>[])
              Semantics(
                label: Copy.signInAgainTo(box.emailAddress),
                button: true,
                child: FilledButton(
                  onPressed: () => _reconnect(api, browser, box.mailboxId),
                  child: Text(Copy.signInAgainTo(box.emailAddress)),
                ),
              ),
          ],
        ),
      ),
    );
  }

  Widget _buildLoadFailed(FeedModel m) {
    return Center(
      child: Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          const Text(Copy.actionFailed, textAlign: TextAlign.center),
          const SizedBox(height: 16),
          Semantics(
            label: Copy.tryAgain,
            button: true,
            child: ElevatedButton(
              onPressed: m.open,
              child: const Text(Copy.tryAgain),
            ),
          ),
        ],
      ),
    );
  }
}

class _OfflineBanner extends StatelessWidget {
  const _OfflineBanner();

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    return Semantics(
      label: Copy.offline,
      child: Material(
        color: scheme.surfaceContainerHighest,
        child: Padding(
          padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 12),
          child: Text(
            Copy.offline,
            textAlign: TextAlign.center,
            style: TextStyle(color: scheme.onSurface),
          ),
        ),
      ),
    );
  }
}

class _MailboxBanner extends StatelessWidget {
  const _MailboxBanner({required this.text, this.action});

  final String text;
  final Widget? action;

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    return Material(
      color: scheme.errorContainer,
      child: Padding(
        padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 8),
        child: Row(
          children: [
            Expanded(
              child: Text(
                text,
                style: TextStyle(color: scheme.onErrorContainer),
              ),
            ),
            ?action,
          ],
        ),
      ),
    );
  }
}
