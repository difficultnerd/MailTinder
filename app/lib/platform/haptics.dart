/// Haptics through the Vibration API where the browser has it.
///
/// The web implementation lives in `haptics_web.dart`; the non-web build uses
/// `haptics_stub.dart`, which keeps `flutter test` on the VM compiling.
library;

export 'haptics_stub.dart' if (dart.library.js_interop) 'haptics_web.dart';

abstract class Haptics {
  /// Whether `navigator.vibrate` exists.
  bool get supported;

  /// A short tap.
  void tap();
}
