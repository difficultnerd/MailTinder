import 'package:flutter/material.dart';

import 'api/api_client.dart';
import 'api/models/session.dart';
import 'copy.dart';
import 'routes.dart';
import 'screens/home/home_shell.dart';
import 'screens/request_invite/request_invite_screen.dart';
import 'screens/sign_in/sign_in_screen.dart';
import 'state/session_model.dart';

class MailTinderApp extends StatefulWidget {
  const MailTinderApp({super.key, required this.session, required this.api});

  final SessionModel session;
  final ApiClient api;

  @override
  State<MailTinderApp> createState() => _MailTinderAppState();
}

class _MailTinderAppState extends State<MailTinderApp> {
  final GlobalKey<NavigatorState> _navigatorKey = GlobalKey<NavigatorState>();
  SessionState? _lastState;

  @override
  void initState() {
    super.initState();
    _lastState = widget.session.session?.state;
    widget.session.addListener(_handleSessionChanged);
    widget.session.refresh();
  }

  @override
  void dispose() {
    widget.session.removeListener(_handleSessionChanged);
    super.dispose();
  }

  void _handleSessionChanged() {
    final currentState = widget.session.session?.state;
    if (_lastState != currentState) {
      _lastState = currentState;
      _navigatorKey.currentState?.popUntil(ModalRoute.withName(Routes.home));
    }
  }

  @override
  Widget build(BuildContext context) {
    return MaterialApp(
      title: Copy.productName,
      navigatorKey: _navigatorKey,
      onGenerateRoute: (settings) =>
          onGenerateRoute(settings, widget.session, widget.api),
      home: ListenableBuilder(
        listenable: widget.session,
        builder: (context, _) {
          final session = widget.session;
          if (session.session == null && session.loading) {
            return Scaffold(
              body: Center(
                child: Semantics(
                  label: Copy.loading,
                  child: const CircularProgressIndicator(),
                ),
              ),
            );
          }

          if (session.session == null && session.lastError != null) {
            return Scaffold(
              body: Center(
                child: Column(
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    const Padding(
                      padding: EdgeInsets.symmetric(horizontal: 24),
                      child: Text(Copy.offline, textAlign: TextAlign.center),
                    ),
                    const SizedBox(height: 16),
                    ElevatedButton(
                      onPressed: session.refresh,
                      child: const Text(Copy.tryAgain),
                    ),
                  ],
                ),
              ),
            );
          }

          final state = session.session?.state;
          return switch (state) {
            SessionState.authenticated => const HomeShell(),
            SessionState.pendingInviteRequest => const RequestInviteScreen(),
            SessionState.anonymous ||
            SessionState.preAuth ||
            null => const SignInScreen(),
          };
        },
      ),
    );
  }
}
