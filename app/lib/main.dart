import 'package:flutter/material.dart';
import 'package:http/http.dart' as http;

import 'api/http_api_client.dart';
import 'app.dart';
import 'state/session_model.dart';

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

  runApp(MailTinderApp(session: sessionModel, api: apiClient));
}
