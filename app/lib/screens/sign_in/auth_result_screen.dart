import 'package:flutter/material.dart';

import '../../api/models/auth.dart';
import '../../copy.dart';
import '../../platform/browser.dart';
import '../../routes.dart';
import '../../state/session_model.dart';
import '../../state/sign_in_model.dart';

/// Handles `/#/auth/result?outcome=...` (S7 3.4): refreshes the session and
/// routes to the right screen, then clears the outcome from the address bar.
class AuthResultScreen extends StatefulWidget {
  const AuthResultScreen({
    super.key,
    required this.outcome,
    required this.session,
    required this.signInModel,
    required this.browser,
  });

  final AuthOutcome outcome;
  final SessionModel session;
  final SignInModel signInModel;
  final Browser browser;

  @override
  State<AuthResultScreen> createState() => _AuthResultScreenState();
}

class _AuthResultScreenState extends State<AuthResultScreen> {
  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addPostFrameCallback((_) {
      _handle();
    });
  }

  Future<void> _handle() async {
    await widget.session.refresh();
    if (!mounted) {
      return;
    }
    final navigator = Navigator.of(context);
    switch (widget.outcome) {
      case AuthOutcome.signedIn:
      case AuthOutcome.joined:
        navigator.pushNamedAndRemoveUntil(Routes.home, (route) => false);
      case AuthOutcome.notInvited:
        navigator.pushNamedAndRemoveUntil(
          Routes.requestInvite,
          (route) => false,
        );
      case AuthOutcome.notRegistered:
      case AuthOutcome.inviteInvalid:
      case AuthOutcome.emailMismatch:
      case AuthOutcome.emailUnverified:
      case AuthOutcome.cancelled:
      case AuthOutcome.failed:
      case AuthOutcome.unknown:
      case AuthOutcome.consentBlocked:
        widget.signInModel.showOutcome(widget.outcome);
        navigator.pushNamedAndRemoveUntil(Routes.signIn, (route) => false);
      case AuthOutcome.linked:
      case AuthOutcome.mailboxLinkedElsewhere:
        navigator.pushNamedAndRemoveUntil(
          Routes.settingsAccounts,
          (route) => false,
          arguments: widget.outcome,
        );
      case AuthOutcome.reconnected:
      case AuthOutcome.steppedUp:
      case AuthOutcome.stepUpWrongAccount:
        navigator.pushNamedAndRemoveUntil(Routes.home, (route) => false);
    }
    widget.browser.replaceAddress('/');
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      body: Center(
        child: Semantics(
          label: Copy.loading,
          child: const CircularProgressIndicator(),
        ),
      ),
    );
  }
}
