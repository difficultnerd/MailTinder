import 'package:flutter/material.dart';

import '../../copy.dart';
import '../../platform/browser.dart';
import '../../state/sign_in_model.dart';

/// Sign-in screen (S9 section 1), all states except v2.
class SignInScreen extends StatefulWidget {
  const SignInScreen({super.key, required this.model, required this.browser});

  final SignInModel model;
  final Browser browser;

  @override
  State<SignInScreen> createState() => _SignInScreenState();
}

class _SignInScreenState extends State<SignInScreen> {
  @override
  Widget build(BuildContext context) {
    return Scaffold(
      body: Center(
        child: ListenableBuilder(
          listenable: widget.model,
          builder: (context, _) {
            final model = widget.model;
            final redirecting = model.status == SignInStatus.redirecting;
            final errorText = model.errorText;

            return SingleChildScrollView(
              padding: const EdgeInsets.symmetric(horizontal: 24, vertical: 32),
              child: Column(
                mainAxisSize: MainAxisSize.min,
                children: [
                  const Text(
                    Copy.productName,
                    style: TextStyle(fontSize: 28, fontWeight: FontWeight.bold),
                  ),
                  const SizedBox(height: 12),
                  const Text(Copy.tagline, textAlign: TextAlign.center),
                  const SizedBox(height: 24),
                  if (model.arrivedFromInvite) ...[
                    const Text(Copy.invited, textAlign: TextAlign.center),
                    const SizedBox(height: 16),
                  ],
                  if (errorText != null) ...[
                    Text(errorText, textAlign: TextAlign.center),
                    const SizedBox(height: 16),
                  ],
                  SizedBox(
                    width: double.infinity,
                    child: ElevatedButton(
                      onPressed: redirecting ? null : model.continueWithGoogle,
                      child: redirecting
                          ? const SizedBox(
                              width: 20,
                              height: 20,
                              child: CircularProgressIndicator(strokeWidth: 2),
                            )
                          : const Text(Copy.continueWithGoogle),
                    ),
                  ),
                  const SizedBox(height: 12),
                  Text(
                    Copy.scopesExplainer,
                    textAlign: TextAlign.center,
                    style: Theme.of(context).textTheme.bodySmall,
                  ),
                  const SizedBox(height: 12),
                  TextButton(
                    onPressed: () {
                      widget.browser.openExternal(Uri.parse('/privacy.html'));
                    },
                    child: const Text(Copy.privacyNotice),
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
