import 'dart:js_interop';
import 'dart:js_interop_unsafe';

import 'package:web/web.dart' as web;

import 'haptics.dart';

/// Web [Haptics]: `navigator.vibrate(15)` when the function exists.
class HapticsImpl implements Haptics {
  const HapticsImpl();

  @override
  bool get supported => web.window.navigator.has('vibrate');

  @override
  void tap() {
    if (!supported) return;
    try {
      web.window.navigator.vibrate(15.toJS);
    } on Object {
      // Ignored: vibration is a nicety.
    }
  }
}
