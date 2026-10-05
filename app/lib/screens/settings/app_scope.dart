import 'package:flutter/widgets.dart';

import '../../api/api_client.dart';
import '../../platform/browser.dart';
import '../../state/play_prefs.dart';
import '../../state/session_model.dart';
import '../../state/step_up_controller.dart';

/// The app-wide dependencies the Settings screens need, supplied above the
/// Navigator so tab content and pushed routes both reach them.
class AppScope extends InheritedWidget {
  const AppScope({
    super.key,
    required this.session,
    required this.api,
    required this.browser,
    required this.stepUp,
    required this.playPrefs,
    required super.child,
  });

  final SessionModel session;
  final ApiClient api;
  final Browser browser;
  final StepUpController stepUp;
  final PlayPrefs playPrefs;

  static AppScope? maybeOf(BuildContext context) =>
      context.dependOnInheritedWidgetOfExactType<AppScope>();

  @override
  bool updateShouldNotify(AppScope oldWidget) =>
      session != oldWidget.session ||
      api != oldWidget.api ||
      browser != oldWidget.browser ||
      stepUp != oldWidget.stepUp ||
      playPrefs != oldWidget.playPrefs;
}
