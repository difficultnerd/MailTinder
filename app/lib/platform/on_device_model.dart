/// The browser's built-in language model, when it has one.
///
/// The web implementation lives in `on_device_model_web.dart`; the non-web
/// build uses `on_device_model_stub.dart`, which keeps `flutter test` on the
/// VM compiling.
library;

export 'on_device_model_stub.dart'
    if (dart.library.js_interop) 'on_device_model_web.dart';

abstract class OnDeviceModel {
  /// True only when the model is ready without a download prompt.
  Future<bool> isAvailable();

  /// Runs one prompt on the device. Null when the model gives no reply.
  Future<String?> prompt(
    String text, {
    Duration timeout = const Duration(milliseconds: 800),
  });
}
