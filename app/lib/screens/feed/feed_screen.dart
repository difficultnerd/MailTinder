import 'package:flutter/material.dart';

import '../../api/api_client.dart';
import '../../api/models/auth.dart';
import '../../api/models/feed.dart';
import '../../api/models/session.dart';
import '../../copy.dart';
import '../../platform/browser.dart';
import '../../state/feed_model.dart';
import '../../state/session_model.dart';
import 'card_view.dart';
import 'divider_card.dart';

/// The Feed tab (S9 section 3): one card in focus with the next behind it,
/// plus every non-swipe Feed state. Swipes arrive in T-1002b.
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
  });

  final FeedModel? model;
  final SessionModel? session;
  final ApiClient? api;
  final Browser? browser;

  @override
  State<FeedScreen> createState() => _FeedScreenState();
}

class _FeedScreenState extends State<FeedScreen> {
  @override
  void initState() {
    super.initState();
    widget.model?.open();
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

    return ListenableBuilder(
      listenable: model,
      builder: (context, _) {
        return Column(
          children: [
            if (model.offline) const _OfflineBanner(),
            for (final err in model.mailboxErrors)
              ..._mailboxBanners(err, session, api, browser),
            Expanded(child: _buildBody(context, model, session, api, browser)),
          ],
        );
      },
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

  Widget _buildReady(BuildContext context, FeedModel m, SessionModel session) {
    final current = m.current;
    final next = m.next;
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
                child: Stack(
                  children: [
                    if (next is CardItem)
                      Positioned.fill(
                        child: _backgroundCard(next.card, session),
                      ),
                    Positioned.fill(child: _currentCard(current, session)),
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

  Widget _currentCard(FeedItem? current, SessionModel session) {
    if (current is CardItem) {
      return CardView(
        card: current.card,
        mailboxAddress: _mailboxAddress(session, current.card.mailboxId),
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
