import 'package:flutter/material.dart';

import 'api/api_client.dart';
import 'api/models/session.dart';
import 'screens/home/home_shell.dart';
import 'screens/request_invite/request_invite_screen.dart';
import 'screens/sign_in/sign_in_screen.dart';
import 'state/session_model.dart';

abstract final class Routes {
  static const home = '/';
  static const signIn = '/sign-in';
  static const requestInvite = '/request-invite';
}

Route<Object?>? onGenerateRoute(
  RouteSettings settings,
  SessionModel session,
  ApiClient api,
) {
  final uri = Uri.parse(settings.name ?? Routes.home);
  final path = uri.path.isEmpty ? Routes.home : uri.path;

  return MaterialPageRoute<Object?>(
    settings: settings,
    builder: (context) {
      switch (path) {
        case Routes.signIn:
          return const SignInScreen();
        case Routes.requestInvite:
          return const RequestInviteScreen();
        case Routes.home:
        default:
          final state = session.session?.state;
          return switch (state) {
            SessionState.authenticated => const HomeShell(),
            SessionState.pendingInviteRequest => const RequestInviteScreen(),
            SessionState.anonymous ||
            SessionState.preAuth ||
            null => const SignInScreen(),
          };
      }
    },
  );
}
