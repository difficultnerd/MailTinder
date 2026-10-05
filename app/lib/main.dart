import 'package:flutter/material.dart';
import 'package:http/http.dart' as http;

import 'api/http_api_client.dart';
import 'app.dart';
import 'platform/browser.dart';
import 'state/session_model.dart';
import 'state/sign_in_model.dart';
import 'state/step_up_controller.dart';

void main() {
  // API base URL: override with --dart-define=API_ORIGIN=http://host:port,
  // otherwise default to the same origin the app is served from.
  const apiOrigin = String.fromEnvironment('API_ORIGIN');
  final origin = apiOrigin.isNotEmpty ? Uri.parse(apiOrigin) : Uri.base;
  late final SessionModel sessionModel;

  final apiClient = HttpApiClient(
    client: http.Client(),
    origin: origin,
    onUnauthenticated: () {
      sessionModel.wipe();
      sessionModel.refresh();
    },
  );

  sessionModel = SessionModel(api: apiClient);
  final browser = const BrowserImpl();
  final signInModel = SignInModel(api: apiClient, browser: browser);
  final stepUp = StepUpController(
    api: apiClient,
    session: sessionModel,
    browser: browser,
  );

  runApp(
    MailTinderApp(
      session: sessionModel,
      api: apiClient,
      signInModel: signInModel,
      browser: browser,
      stepUp: stepUp,
    ),
  );
}
