import 'package:flutter/material.dart';

import '../../api/api_client.dart';
import '../../api/models/auth.dart';
import '../../api/models/session.dart';
import '../../copy.dart';
import '../../platform/browser.dart';
import '../../routes.dart';
import 'app_scope.dart';

/// S9 7.1: list, Add Gmail, Sign in again, Disconnect.
///
/// [outcome] is the callback outcome handed over by the auth result screen.
class ConnectedAccountsScreen extends StatefulWidget {
  const ConnectedAccountsScreen({super.key, this.outcome});

  final AuthOutcome? outcome;

  @override
  State<ConnectedAccountsScreen> createState() =>
      _ConnectedAccountsScreenState();
}

class _ConnectedAccountsScreenState extends State<ConnectedAccountsScreen> {
  List<Mailbox>? _mailboxes;
  bool _failed = false;
  bool _announced = false;

  AppScope get _scope => AppScope.maybeOf(context)!;

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    if (_announced) {
      return;
    }
    _announced = true;
    _load();
    final message = switch (widget.outcome) {
      AuthOutcome.linked => Copy.mailboxAdded,
      AuthOutcome.mailboxLinkedElsewhere => Copy.mailboxLinkedElsewhere,
      _ => null,
    };
    if (message != null) {
      final messenger = ScaffoldMessenger.of(context);
      WidgetsBinding.instance.addPostFrameCallback((_) {
        messenger.showSnackBar(SnackBar(content: Text(message)));
      });
    }
  }

  Future<void> _load() async {
    try {
      final list = await _scope.api.listMailboxes();
      if (!mounted) {
        return;
      }
      setState(() {
        _mailboxes = list;
        _failed = false;
      });
    } on Object {
      if (!mounted) {
        return;
      }
      setState(() => _failed = true);
    }
  }

  void _toast(String text) {
    ScaffoldMessenger.of(context).showSnackBar(SnackBar(content: Text(text)));
  }

  void _go(Uri url) {
    final target = safeNavigationTarget(
      url.toString(),
      allowLoopbackHttp: kE2eBuild,
    );
    if (target == null) {
      _toast(Copy.actionFailed);
      return;
    }
    _scope.browser.assign(target);
  }

  Future<void> _addGmail() async {
    final scope = _scope;
    try {
      final url = await scope.stepUp.run<Uri>(
        waitingActionLabel: Copy.stepUpAddGmail,
        action: () => scope.api.startAuth(intent: AuthIntent.link),
      );
      if (url != null && mounted) {
        _go(url);
      }
    } on Object {
      if (mounted) {
        _toast(Copy.actionFailed);
      }
    }
  }

  Future<void> _signInAgain(Mailbox m) async {
    try {
      final url = await _scope.api.startAuth(
        intent: AuthIntent.reconnect,
        mailboxId: m.mailboxId,
      );
      if (mounted) {
        _go(url);
      }
    } on Object {
      if (mounted) {
        _toast(Copy.actionFailed);
      }
    }
  }

  Future<void> _showOnlyMailbox() {
    return showDialog<void>(
      context: context,
      builder: (dialogContext) => AlertDialog(
        content: const Text(Copy.onlyMailbox),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(dialogContext).pop(),
            child: const Text(Copy.close),
          ),
          TextButton(
            onPressed: () {
              Navigator.of(dialogContext).pop();
              Navigator.of(context).pushNamed(Routes.settingsAccount);
            },
            child: const Text(Copy.goToAccount),
          ),
        ],
      ),
    );
  }

  Future<void> _disconnect(Mailbox m) async {
    final scope = _scope;
    if ((_mailboxes?.length ?? 0) <= 1) {
      await _showOnlyMailbox();
      return;
    }
    final ok = await showDialog<bool>(
      context: context,
      builder: (dialogContext) => AlertDialog(
        content: Text(Copy.disconnectQuestion(m.emailAddress)),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(dialogContext).pop(false),
            child: const Text(Copy.cancel),
          ),
          TextButton(
            onPressed: () => Navigator.of(dialogContext).pop(true),
            child: const Text(Copy.disconnect),
          ),
        ],
      ),
    );
    if (ok != true || !mounted) {
      return;
    }
    try {
      final done = await scope.stepUp.run<bool>(
        waitingActionLabel: Copy.stepUpDisconnect(m.emailAddress),
        action: () async {
          await scope.api.disconnectMailbox(m.mailboxId);
          return true;
        },
      );
      if (done == true) {
        await _load();
        await scope.session.refresh();
      }
    } on ApiException catch (e) {
      if (!mounted) {
        return;
      }
      if (e.code == 'last_mailbox') {
        await _showOnlyMailbox();
      } else if (e.code == 'app_folder_move_failed') {
        _toast(Copy.appFolderMoveFailed);
      } else {
        _toast(Copy.actionFailed);
      }
    } on Object {
      if (mounted) {
        _toast(Copy.actionFailed);
      }
    }
  }

  @override
  Widget build(BuildContext context) {
    final mailboxes = _mailboxes;
    Widget body;
    if (mailboxes == null) {
      body = Center(
        child: _failed
            ? Column(
                mainAxisSize: MainAxisSize.min,
                children: [
                  const Text(Copy.genericError),
                  TextButton(
                    onPressed: _load,
                    child: const Text(Copy.tryAgain),
                  ),
                ],
              )
            : const CircularProgressIndicator(),
      );
    } else {
      body = ListView(
        children: [
          for (final m in mailboxes)
            ListTile(
              title: Text(m.provider == 'gmail' ? 'Gmail' : m.provider),
              subtitle: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(m.emailAddress),
                  Text(
                    m.status == MailboxStatus.needsSignIn
                        ? Copy.statusNeedsSignIn
                        : Copy.statusConnected,
                  ),
                ],
              ),
              trailing: Wrap(
                children: [
                  if (m.status == MailboxStatus.needsSignIn)
                    TextButton(
                      onPressed: () => _signInAgain(m),
                      child: const Text(Copy.signInAgain),
                    ),
                  TextButton(
                    onPressed: () => _disconnect(m),
                    child: const Text(Copy.disconnect),
                  ),
                ],
              ),
            ),
          Padding(
            padding: const EdgeInsets.all(16),
            child: Align(
              alignment: Alignment.centerLeft,
              child: ElevatedButton(
                onPressed: _addGmail,
                child: const Text(Copy.addGmail),
              ),
            ),
          ),
        ],
      );
    }
    return Scaffold(
      appBar: AppBar(title: const Text(Copy.settingsConnectedAccounts)),
      body: body,
    );
  }
}
