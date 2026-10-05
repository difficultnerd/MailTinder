import 'dart:async';

import 'package:flutter/material.dart';

import '../../api/api_client.dart';
import '../../api/models/auth.dart';
import '../../api/models/needs_attention.dart';
import '../../api/models/session.dart';
import '../../copy.dart';
import '../../format.dart';
import '../../platform/browser.dart';
import '../../state/needs_attention_model.dart';
import '../../state/session_model.dart';

/// The Needs Attention tab (S9 section 6, S2 NA-01): open items newest first
/// with sender, mailbox badge, reason copy, when it was raised, and the
/// actions "Open unsubscribe page", "Done" and "Dismiss".
///
/// When [model] is null the screen renders the placeholder: the HomeShell
/// builds it without dependencies until a later task wires the live model.
class NeedsAttentionScreen extends StatefulWidget {
  const NeedsAttentionScreen({
    super.key,
    this.model,
    this.session,
    this.api,
    this.browser,
  });

  final NeedsAttentionModel? model;
  final SessionModel? session;
  final ApiClient? api;
  final Browser? browser;

  @override
  State<NeedsAttentionScreen> createState() => _NeedsAttentionScreenState();
}

class _NeedsAttentionScreenState extends State<NeedsAttentionScreen> {
  Timer? _refresh;

  @override
  void initState() {
    super.initState();
    final model = widget.model;
    if (model != null) {
      model.load();
      // Reload while the tab is open (S9 section 6 `[DEFAULT]`).
      _refresh = Timer.periodic(model.refreshEvery, (_) => model.load());
    }
  }

  @override
  void dispose() {
    _refresh?.cancel();
    super.dispose();
  }

  @override
  void didUpdateWidget(NeedsAttentionScreen oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.model == widget.model) return;
    _refresh?.cancel();
    _refresh = null;
    final model = widget.model;
    if (model == null) return;
    // Outside the build phase: load notifies listeners straight away.
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (!mounted) return;
      model.load();
      _refresh = Timer.periodic(model.refreshEvery, (_) => model.load());
    });
  }

  @override
  Widget build(BuildContext context) {
    final model = widget.model;
    if (model == null) {
      return const Center(child: Text(Copy.tabNeedsAttention));
    }

    return ListenableBuilder(
      listenable: model,
      builder: (context, _) {
        return switch (model.state) {
          NaLoadState.loading => Center(
            child: Semantics(
              label: Copy.loading,
              child: const CircularProgressIndicator(),
            ),
          ),
          NaLoadState.failed => Center(
            child: Column(
              mainAxisSize: MainAxisSize.min,
              children: [
                const Text(Copy.actionFailed, textAlign: TextAlign.center),
                const SizedBox(height: 16),
                Semantics(
                  label: Copy.tryAgain,
                  button: true,
                  child: ElevatedButton(
                    onPressed: model.load,
                    child: const Text(Copy.tryAgain),
                  ),
                ),
              ],
            ),
          ),
          NaLoadState.ready =>
            model.items.isEmpty
                ? const Center(child: Text(Copy.nothingNeedsYou))
                : ListView(
                    children: [
                      for (final item in model.items) _row(model, item),
                    ],
                  ),
        };
      },
    );
  }

  Widget _row(NeedsAttentionModel model, NeedsAttentionItem item) {
    final address = _address(item.mailboxId);
    final reason = effectiveNaReason(
      item.reason,
      mailboxNeedsSignIn: _mailboxNeedsSignIn(item.mailboxId),
    );
    final link = item.link;

    return Padding(
      padding: const EdgeInsets.fromLTRB(16, 12, 16, 4),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(item.senderDisplay),
          if (address.isNotEmpty) Text(address),
          Text(_reasonCopy(reason, item.senderDisplay, address)),
          Text(formatDateTime(item.createdAt)),
          Wrap(
            spacing: 8,
            children: [
              if (reason == NaReason.mailboxNeedsSignIn)
                _action(
                  label: Copy.signInAgain,
                  onPressed: () => unawaited(_signInAgain(item)),
                )
              else if (link != null && naReasonAllowsOpen(reason))
                _action(
                  label: Copy.openUnsubscribePage,
                  onPressed: () => _openPage(link),
                ),
              _action(
                label: Copy.done,
                onPressed: () => unawaited(_resolve(model, item)),
              ),
              _action(
                label: Copy.dismiss,
                onPressed: () => unawaited(_dismiss(model, item)),
              ),
            ],
          ),
        ],
      ),
    );
  }

  Widget _action({required String label, required VoidCallback onPressed}) {
    return Semantics(
      label: label,
      button: true,
      child: TextButton(onPressed: onPressed, child: Text(label)),
    );
  }

  String _address(String mailboxId) {
    for (final m in widget.session?.session?.mailboxes ?? const <Mailbox>[]) {
      if (m.mailboxId == mailboxId) return m.emailAddress;
    }
    return '';
  }

  bool _mailboxNeedsSignIn(String mailboxId) {
    for (final m in widget.session?.session?.mailboxes ?? const <Mailbox>[]) {
      if (m.mailboxId == mailboxId) {
        return m.status == MailboxStatus.needsSignIn;
      }
    }
    return false;
  }

  String _reasonCopy(NaReason reason, String sender, String address) {
    return switch (reason) {
      NaReason.httpsOnlyUnsubscribe => Copy.naHttpsOnlyUnsubscribe,
      NaReason.oneClickRedirect => Copy.naOneClickRedirect,
      NaReason.oneClickAddressRefused => Copy.naOneClickAddressRefused,
      NaReason.unsubscribeIgnored => Copy.naUnsubscribeIgnored,
      NaReason.unsubscribeFailed => Copy.naUnsubscribeFailed,
      NaReason.jobExpired => Copy.naJobExpired,
      NaReason.mailboxNeedsSignIn => Copy.naMailboxNeedsSignIn(sender, address),
      NaReason.other => Copy.naOther,
    };
  }

  /// Opens only the item's own server-provided link, https only (UN-04 AC6).
  void _openPage(Uri link) {
    final browser = widget.browser;
    if (browser == null) return;
    final target = safeNavigationTarget(
      link.toString(),
      allowLoopbackHttp: false,
    );
    if (target == null) return;
    browser.openExternal(target);
  }

  Future<void> _signInAgain(NeedsAttentionItem item) async {
    final api = widget.api;
    final browser = widget.browser;
    if (api == null || browser == null) return;
    try {
      final url = await api.startAuth(
        intent: AuthIntent.reconnect,
        mailboxId: item.mailboxId,
      );
      final target = safeNavigationTarget(
        url.toString(),
        allowLoopbackHttp: false,
      );
      if (target != null) browser.assign(target);
    } on Object {
      _show(Copy.actionFailed);
    }
  }

  Future<void> _resolve(
    NeedsAttentionModel model,
    NeedsAttentionItem item,
  ) async {
    await model.resolve(item);
    final error = model.actionError;
    if (error != null) _show(error);
  }

  Future<void> _dismiss(
    NeedsAttentionModel model,
    NeedsAttentionItem item,
  ) async {
    await model.dismiss(item);
    final error = model.actionError;
    if (error != null) _show(error);
  }

  void _show(String message) {
    if (!mounted) return;
    ScaffoldMessenger.of(
      context,
    ).showSnackBar(SnackBar(content: Text(message)));
  }
}
