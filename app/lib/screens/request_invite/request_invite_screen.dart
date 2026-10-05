import 'package:flutter/material.dart';

import '../../api/api_client.dart';
import '../../copy.dart';
import '../../routes.dart';
import '../../state/session_model.dart';
import '../../state/sign_in_model.dart';

/// Request invite screen (S9 section 2), shown in the `pending_invite_request`
/// session state.
class RequestInviteScreen extends StatefulWidget {
  const RequestInviteScreen({
    super.key,
    required this.session,
    required this.api,
    required this.signInModel,
  });

  final SessionModel session;
  final ApiClient api;
  final SignInModel signInModel;

  @override
  State<RequestInviteScreen> createState() => _RequestInviteScreenState();
}

class _RequestInviteScreenState extends State<RequestInviteScreen> {
  bool _submitted = false;
  String? _errorText;

  Future<void> _requestInvite() async {
    setState(() {
      _errorText = null;
    });
    try {
      await widget.api.createInviteRequest();
      if (!mounted) {
        return;
      }
      setState(() {
        _submitted = true;
      });
    } on ApiException catch (e) {
      if (!mounted) {
        return;
      }
      setState(() {
        _errorText = e.code == 'rate_limited'
            ? Copy.tooManyRequests
            : Copy.actionFailed;
      });
    } on Object {
      if (!mounted) {
        return;
      }
      setState(() {
        _errorText = Copy.actionFailed;
      });
    }
  }

  Future<void> _useDifferentAccount() async {
    try {
      await widget.api.signOut();
    } catch (_) {
      // Ignore sign-out errors: still wipe memory and show Sign-in.
    }
    widget.session.wipe();
    widget.signInModel.resetToDefault();
    if (!mounted) {
      return;
    }
    Navigator.of(
      context,
    ).pushNamedAndRemoveUntil(Routes.signIn, (route) => false);
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      body: Center(
        child: ListenableBuilder(
          listenable: widget.session,
          builder: (context, _) {
            final email = widget.session.session?.pendingInviteEmail ?? '';
            return SingleChildScrollView(
              padding: const EdgeInsets.symmetric(horizontal: 24, vertical: 32),
              child: Column(
                mainAxisSize: MainAxisSize.min,
                children: [
                  Text(email, textAlign: TextAlign.center),
                  const SizedBox(height: 12),
                  const Text(
                    Copy.inviteOnlyExplainer,
                    textAlign: TextAlign.center,
                  ),
                  const SizedBox(height: 16),
                  if (_submitted) ...[
                    const Text(Copy.requestSent, textAlign: TextAlign.center),
                  ] else ...[
                    if (_errorText != null) ...[
                      Text(_errorText!, textAlign: TextAlign.center),
                      const SizedBox(height: 16),
                    ],
                    SizedBox(
                      width: double.infinity,
                      child: ElevatedButton(
                        onPressed: _requestInvite,
                        child: const Text(Copy.requestInvite),
                      ),
                    ),
                  ],
                  const SizedBox(height: 8),
                  TextButton(
                    onPressed: _useDifferentAccount,
                    child: const Text(Copy.useDifferentAccount),
                  ),
                ],
              ),
            );
          },
        ),
      ),
    );
  }
}
