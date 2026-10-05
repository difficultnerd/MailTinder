import 'dart:async';

import 'package:flutter/material.dart';

import '../../api/api_client.dart';
import '../../copy.dart';
import '../../routes.dart';
import 'app_scope.dart';

/// S9 7.5: Sounds, Sign out and Delete account.
class AccountScreen extends StatelessWidget {
  const AccountScreen({super.key});

  @override
  Widget build(BuildContext context) {
    final scope = AppScope.maybeOf(context);
    if (scope == null) {
      return const Scaffold(body: SizedBox.shrink());
    }
    return Scaffold(
      appBar: AppBar(title: const Text(Copy.settingsAccount)),
      body: ListView(
        children: [
          ListenableBuilder(
            listenable: scope.playPrefs,
            builder: (context, _) => SwitchListTile(
              title: const Text(Copy.sounds),
              value: scope.playPrefs.soundsOn,
              onChanged: (v) => scope.playPrefs.soundsOn = v,
            ),
          ),
          ListTile(
            title: const Text(Copy.signOut),
            onTap: () => _signOut(context, scope),
          ),
          ListTile(
            title: Text(
              Copy.deleteAccount,
              style: TextStyle(color: Theme.of(context).colorScheme.error),
            ),
            onTap: () => _deleteAccount(context, scope),
          ),
        ],
      ),
    );
  }

  Future<void> _signOut(BuildContext context, AppScope scope) async {
    final navigator = Navigator.of(context);
    // SessionModel.signOut wipes in a finally block, so a network failure
    // still clears every in-memory model (ASVS V14.3.1).
    await scope.session.signOut();
    unawaited(
      navigator.pushNamedAndRemoveUntil(Routes.signIn, (route) => false),
    );
  }

  Future<bool> _confirm(
    BuildContext context,
    String text,
    String actionLabel,
  ) async {
    final result = await showDialog<bool>(
      context: context,
      builder: (context) => AlertDialog(
        content: Text(text),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(context).pop(false),
            child: const Text(Copy.cancel),
          ),
          TextButton(
            onPressed: () => Navigator.of(context).pop(true),
            child: Text(actionLabel),
          ),
        ],
      ),
    );
    return result ?? false;
  }

  Future<void> _deleteAccount(BuildContext context, AppScope scope) async {
    final navigator = Navigator.of(context);
    final messenger = ScaffoldMessenger.of(context);
    if (!await _confirm(
      context,
      Copy.deleteAccountExplain,
      Copy.continueButton,
    )) {
      return;
    }
    if (!context.mounted) {
      return;
    }
    if (!await _confirm(
      context,
      Copy.deleteAccountConfirm,
      Copy.deleteAccount,
    )) {
      return;
    }
    DeleteAccountResult? result;
    try {
      result = await scope.stepUp.run<DeleteAccountResult>(
        waitingActionLabel: Copy.stepUpDeleteAccount,
        action: scope.api.deleteAccount,
      );
    } on Object {
      messenger.showSnackBar(const SnackBar(content: Text(Copy.actionFailed)));
      return;
    }
    if (result == null) {
      return;
    }
    if (result.appFoldersNotDeleted.isNotEmpty && context.mounted) {
      await showDialog<void>(
        context: context,
        builder: (context) => AlertDialog(
          content: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              const Text(Copy.appFoldersNotDeleted),
              const SizedBox(height: 8),
              for (final f in result!.appFoldersNotDeleted)
                Text(f.emailAddress),
            ],
          ),
          actions: [
            TextButton(
              onPressed: () => Navigator.of(context).pop(),
              child: const Text(Copy.close),
            ),
          ],
        ),
      );
    }
    scope.session.wipe();
    unawaited(
      navigator.pushNamedAndRemoveUntil(Routes.signIn, (route) => false),
    );
    messenger.showSnackBar(const SnackBar(content: Text(Copy.accountDeleted)));
  }
}
