import 'package:flutter/material.dart';

import 'api/api_client.dart';
import 'api/models/auth.dart';
import 'api/models/session.dart';
import 'platform/browser.dart';
import 'screens/home/home_shell.dart';
import 'screens/request_invite/request_invite_screen.dart';
import 'screens/settings/account_screen.dart';
import 'screens/settings/connected_accounts_screen.dart';
import 'screens/sign_in/auth_result_screen.dart';
import 'screens/sign_in/sign_in_screen.dart';
import 'state/session_model.dart';
import 'state/sign_in_model.dart';

abstract final class Routes {
  static const home = '/';
  static const signIn = '/sign-in';
  static const invite = '/invite';
  static const authResult = '/auth/result';
  static const requestInvite = '/request-invite';
  static const settingsAccounts = '/settings/accounts';
  static const settingsAccount = '/settings/account';
}

Route<Object?>? onGenerateRoute(
  RouteSettings settings,
  SessionModel session,
  ApiClient api,
  SignInModel signInModel,
  Browser browser,
) {
  final uri = Uri.parse(settings.name ?? Routes.home);
  final path = uri.path.isEmpty ? Routes.home : uri.path;

  return MaterialPageRoute<Object?>(
    settings: settings,
    builder: (context) {
      switch (path) {
        case Routes.signIn:
          // Signed out (AU-07 AC1): show the default state whenever the
          // session is not authenticated and no outcome or invite token is
          // being shown.
          final state = session.session?.state;
          if (state != SessionState.authenticated) {
            signInModel.resetToDefault();
          }
          return SignInScreen(model: signInModel, browser: browser);
        case Routes.invite:
          final token = uri.queryParameters['t'];
          if (token != null) {
            // Defer until after the frame: acceptInviteToken notifies
            // listeners, which must not happen during the route build.
            WidgetsBinding.instance.addPostFrameCallback((_) {
              signInModel.acceptInviteToken(token);
            });
          }
          browser.replaceAddress('/');
          return SignInScreen(model: signInModel, browser: browser);
        case Routes.authResult:
          final outcome = parseAuthOutcome(uri.queryParameters['outcome']);
          return AuthResultScreen(
            outcome: outcome,
            session: session,
            signInModel: signInModel,
            browser: browser,
          );
        case Routes.requestInvite:
          return RequestInviteScreen(
            session: session,
            api: api,
            signInModel: signInModel,
          );
        case Routes.settingsAccounts:
          final args = settings.arguments;
          return ConnectedAccountsScreen(
            outcome: args is AuthOutcome ? args : null,
          );
        case Routes.settingsAccount:
          return const AccountScreen();
        case Routes.home:
        default:
          final state = session.session?.state;
          return switch (state) {
            SessionState.authenticated => const HomeShell(),
            SessionState.pendingInviteRequest => RequestInviteScreen(
              session: session,
              api: api,
              signInModel: signInModel,
            ),
            SessionState.anonymous ||
            SessionState.preAuth ||
            null => SignInScreen(model: signInModel, browser: browser),
          };
      }
    },
  );
}
