/// Browser navigation and window helpers, without any browser storage.
///
/// The web implementation lives in `browser_web.dart` and uses `package:web`.
/// `package:web` does not compile on the VM, so it is imported only behind the
/// conditional export below; the non-web build uses `browser_stub.dart`, which
/// keeps `flutter test` on the VM compiling.
library;

export 'browser_stub.dart' if (dart.library.js_interop) 'browser_web.dart';

/// True only in e2e builds (T-1101a), where loopback http is allowed.
const bool kE2eBuild = bool.fromEnvironment('MT_E2E');

/// Page navigation and new tabs, without browser storage.
abstract class Browser {
  /// Same-tab navigation (window.location.assign).
  void assign(Uri url);

  /// New tab with 'noopener,noreferrer'.
  void openExternal(Uri url);

  /// Opens an about:blank popup; call synchronously in a tap handler.
  /// Returns null if the popup was blocked.
  BrowserPopup? openPopup(String name);

  /// Whether this window is the step-up popup (window.name == 'mt_step_up').
  bool get isPopupWindow;

  /// window.close().
  void closeWindow();

  /// history.replaceState to '#$hashPath'.
  void replaceAddress(String hashPath);

  /// Saves [bytes] as a download named [fileName] (Blob plus a clicked
  /// `<a download>`, revoked afterwards).
  void saveFile(List<int> bytes, String fileName, String mimeType);
}

/// A popup window opened by [Browser.openPopup].
abstract class BrowserPopup {
  void navigate(Uri url);
  void close();
}

/// Returns the URI only if it is https, or http on localhost/127.0.0.1 when
/// [allowLoopbackHttp] is true. Returns null otherwise.
Uri? safeNavigationTarget(String raw, {required bool allowLoopbackHttp}) {
  final uri = Uri.tryParse(raw);
  if (uri == null) {
    return null;
  }
  if (uri.scheme == 'https') {
    return uri;
  }
  if (allowLoopbackHttp &&
      uri.scheme == 'http' &&
      (uri.host == 'localhost' || uri.host == '127.0.0.1')) {
    return uri;
  }
  return null;
}
