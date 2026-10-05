import 'dart:async';

import 'package:flutter/foundation.dart';

import '../api/api_client.dart';
import '../api/models/auth.dart';
import '../platform/browser.dart';
import 'session_model.dart';

typedef StepUpAction<T> = Future<T> Function();

enum StepUpStatus { idle, prompting, redirecting, popupBlocked }

/// Drives the "Confirm it's you" step-up overlay (S9 section 1.1, S7 3.6).
///
/// Any screen wraps a sensitive call in [run]. When the server answers
/// `403 step_up_required` the overlay appears, the user signs in again in a
/// popup, and once the session reports a fresh step-up the waiting request is
/// re-sent once without the user tapping again.
///
/// Nothing about the waiting action is stored in browser storage; the action
/// closure lives in memory only (S5, ASVS V14.3.3). No `postMessage` or
/// `BroadcastChannel` listeners are added (ASVS V3.5.5).
class StepUpController extends ChangeNotifier {
  StepUpController({
    required ApiClient api,
    required SessionModel session,
    required Browser browser,
    DateTime Function()? now,
    Duration pollEvery = const Duration(seconds: 2),
    Duration giveUpAfter = const Duration(minutes: 5),
  }) : _api = api,
       _session = session,
       _browser = browser,
       _now = now ?? DateTime.now,
       _pollEvery = pollEvery,
       _giveUpAfter = giveUpAfter;

  final ApiClient _api;
  final SessionModel _session;
  final Browser _browser;
  final DateTime Function() _now;
  final Duration _pollEvery;
  final Duration _giveUpAfter;

  StepUpStatus _status = StepUpStatus.idle;
  String? _waitingActionLabel;
  DateTime? _baseline;
  DateTime? _deadline;
  Timer? _pollTimer;
  BrowserPopup? _popup;
  Completer<bool>? _stepUpResult;
  int _notConfirmedCount = 0;
  Future<Object?>? _activeRun;

  StepUpStatus get status => _status;
  String? get waitingActionLabel => _waitingActionLabel;

  /// Number of step-up flows that ended without confirmation since the last
  /// clear. Used by [StepUpOverlayHost] to show the "Not confirmed" SnackBar.
  int get notConfirmedCount => _notConfirmedCount;

  /// Runs [action]. On `ApiException` with code `step_up_required` the overlay
  /// is shown; after the session reports a fresh step-up, [action] is run
  /// exactly once more and its result returned. Returns null on cancel or
  /// when confirmation does not arrive. Other errors are rethrown.
  Future<T?> run<T>({
    required String waitingActionLabel,
    required StepUpAction<T> action,
  }) async {
    // Only one step-up runs at a time: a second `run` while one is open waits
    // for the first to finish, then starts its own.
    while (_activeRun != null) {
      await _activeRun;
    }
    final gate = Completer<Object?>();
    _activeRun = gate.future;
    try {
      try {
        return await action();
      } on ApiException catch (e) {
        if (e.code != 'step_up_required') {
          rethrow;
        }
      }
      final confirmed = await _startStepUp(waitingActionLabel);
      if (!confirmed) {
        return null;
      }
      // Re-run the same closure so a route that takes an Idempotency-Key gets
      // the same one (S7 3.6). Do not loop if it asks again.
      try {
        return await action();
      } on ApiException catch (e) {
        if (e.code == 'step_up_required') {
          return null;
        }
        rethrow;
      }
    } finally {
      _activeRun = null;
      gate.complete(null);
    }
  }

  /// Must be called synchronously from the button's onPressed so the browser
  /// does not treat the popup as unsolicited.
  void continueWithGoogle() {
    if (_status != StepUpStatus.prompting &&
        _status != StepUpStatus.popupBlocked) {
      return;
    }
    final popup = _browser.openPopup('mt_step_up');
    if (popup == null) {
      _status = StepUpStatus.popupBlocked;
      notifyListeners();
      return;
    }
    _popup = popup;
    _deadline = _now().add(_giveUpAfter);
    _status = StepUpStatus.redirecting;
    notifyListeners();
    unawaited(_drivePopup());
  }

  void cancel() {
    if (_status != StepUpStatus.idle) {
      _completeStepUp(false);
    }
  }

  Future<bool> _startStepUp(String label) async {
    _baseline = _session.session?.stepUpValidUntil;
    _waitingActionLabel = label;
    _popup = null;
    _pollTimer?.cancel();
    _pollTimer = null;
    _status = StepUpStatus.prompting;
    final completer = Completer<bool>();
    _stepUpResult = completer;
    notifyListeners();

    final confirmed = await completer.future;

    _stepUpResult = null;
    _waitingActionLabel = null;
    _status = StepUpStatus.idle;
    notifyListeners();
    return confirmed;
  }

  Future<void> _drivePopup() async {
    _pollTimer ??= Timer.periodic(_pollEvery, (_) => _poll());
    try {
      final url = await _api.startAuth(intent: AuthIntent.stepUp);
      if (_status != StepUpStatus.redirecting) {
        return; // cancelled while the request was in flight
      }
      final target = safeNavigationTarget(
        url.toString(),
        allowLoopbackHttp: false,
      );
      if (target == null) {
        _completeStepUp(false);
        return;
      }
      _popup?.navigate(target);
    } on Object {
      if (_status == StepUpStatus.redirecting) {
        _completeStepUp(false);
      }
    }
  }

  Future<void> _poll() async {
    await _session.refresh();
    if (_status != StepUpStatus.redirecting) {
      return;
    }
    final stepUp = _session.session?.stepUpValidUntil;
    final now = _now();
    final baseline = _baseline;
    final confirmed =
        stepUp != null &&
        stepUp.isAfter(now) &&
        (baseline == null || stepUp.isAfter(baseline));
    if (confirmed) {
      _completeStepUp(true);
      return;
    }
    if (now.isAfter(_deadline!)) {
      _completeStepUp(false);
    }
  }

  void _completeStepUp(bool confirmed) {
    _pollTimer?.cancel();
    _pollTimer = null;
    final completer = _stepUpResult;
    if (completer == null || completer.isCompleted) {
      return;
    }
    if (!confirmed) {
      _notConfirmedCount++;
    }
    final popup = _popup;
    _popup = null;
    if (popup != null) {
      try {
        popup.close();
      } catch (_) {
        // Popup close is best-effort.
      }
    }
    completer.complete(confirmed);
  }
}
