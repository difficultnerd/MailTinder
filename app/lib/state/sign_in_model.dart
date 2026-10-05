import 'package:flutter/foundation.dart';

import '../api/api_client.dart';
import '../api/models/auth.dart';
import '../copy.dart';
import '../platform/browser.dart';

enum SignInStatus { idle, redirecting, error }

/// State for the Sign-in screen: the invite token (memory only), the current
/// status, and the error copy to show.
class SignInModel extends ChangeNotifier {
  SignInModel({
    required ApiClient api,
    required Browser browser,
    bool allowLoopbackHttp = kE2eBuild,
  }) : _api = api,
       _browser = browser,
       _allowLoopbackHttp = allowLoopbackHttp;

  final ApiClient _api;
  final Browser _browser;
  final bool _allowLoopbackHttp;

  SignInStatus _status = SignInStatus.idle;
  String? _errorText;
  String? _inviteToken;
  bool _arrivedFromInvite = false;

  SignInStatus get status => _status;
  String? get errorText => _errorText;
  bool get arrivedFromInvite => _arrivedFromInvite;

  /// Valid invite tokens are 43 to 64 characters of [A-Za-z0-9_-].
  static final RegExp _inviteTokenPattern = RegExp(r'^[A-Za-z0-9_-]{43,64}$');

  /// Validates [raw] and keeps it in memory only. Invalid -> error inviteInvalid.
  void acceptInviteToken(String raw) {
    if (_inviteTokenPattern.hasMatch(raw)) {
      _inviteToken = raw;
      _arrivedFromInvite = true;
      _status = SignInStatus.idle;
      _errorText = null;
    } else {
      _inviteToken = null;
      _arrivedFromInvite = false;
      _status = SignInStatus.error;
      _errorText = Copy.inviteInvalid;
    }
    notifyListeners();
  }

  /// Maps an auth outcome to the Sign-in error copy. Outcomes that route
  /// elsewhere (signedIn, joined, notInvited, linked, ...) reset to default.
  void showOutcome(AuthOutcome outcome) {
    switch (outcome) {
      case AuthOutcome.notRegistered:
        _setError(Copy.notRegistered);
      case AuthOutcome.inviteInvalid:
        _setError(Copy.inviteInvalid);
      case AuthOutcome.emailMismatch:
        _setError(Copy.emailMismatch);
      case AuthOutcome.emailUnverified:
        _setError(Copy.emailUnverified);
      case AuthOutcome.cancelled:
      case AuthOutcome.failed:
      case AuthOutcome.unknown:
      case AuthOutcome.consentBlocked:
        _setError(Copy.signInFailed);
      default:
        resetToDefault();
    }
  }

  /// Clears the invite token and any error, returning to the default state.
  /// Called after sign-out or a 401 wipe.
  void resetToDefault() {
    _inviteToken = null;
    _arrivedFromInvite = false;
    _status = SignInStatus.idle;
    _errorText = null;
    notifyListeners();
  }

  void _setError(String text) {
    _status = SignInStatus.error;
    _errorText = text;
    notifyListeners();
  }

  /// Starts Google OAuth with intent `join`, carrying the invite token when
  /// present. Ignores a second tap while redirecting.
  Future<void> continueWithGoogle() async {
    if (_status == SignInStatus.redirecting) {
      return;
    }
    _status = SignInStatus.redirecting;
    _errorText = null;
    notifyListeners();

    try {
      final url = await _api.startAuth(
        intent: AuthIntent.join,
        inviteToken: _inviteToken,
      );
      final target = safeNavigationTarget(
        url.toString(),
        allowLoopbackHttp: _allowLoopbackHttp,
      );
      if (target == null) {
        _setError(Copy.signInFailed);
        return;
      }
      _browser.assign(target);
      // Stay redirecting: the page navigates away.
    } on ApiException catch (e) {
      if (e.code == 'rate_limited') {
        _setError(Copy.tooManyRequests);
      } else {
        _setError(Copy.signInFailed);
      }
    } on NetworkException {
      _setError(Copy.signInFailed);
    } on Object {
      _setError(Copy.signInFailed);
    }
  }
}
