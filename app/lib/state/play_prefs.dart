import 'package:flutter/foundation.dart';

/// Play preferences. Memory only: nothing goes to browser storage (S5), so
/// Sounds is off again in each new tab (GM-02 AC2).
class PlayPrefs extends ChangeNotifier {
  bool _soundsOn = false;

  bool get soundsOn => _soundsOn;

  set soundsOn(bool v) {
    if (_soundsOn == v) {
      return;
    }
    _soundsOn = v;
    notifyListeners();
  }
}
