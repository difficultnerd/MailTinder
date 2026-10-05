import 'browser.dart';

/// Non-web implementation of [Browser] that throws [UnsupportedError].
///
/// Keeps `flutter test` on the VM compiling; the real behaviour is in
/// `browser_web.dart`, which is selected by the conditional export in
/// `browser.dart`.
class BrowserImpl implements Browser {
  const BrowserImpl();

  @override
  void assign(Uri url) {
    throw UnsupportedError('Browser.assign is not available on this platform');
  }

  @override
  void openExternal(Uri url) {
    throw UnsupportedError(
      'Browser.openExternal is not available on this platform',
    );
  }

  @override
  BrowserPopup? openPopup(String name) {
    throw UnsupportedError(
      'Browser.openPopup is not available on this platform',
    );
  }

  @override
  bool get isPopupWindow => false;

  @override
  void closeWindow() {
    throw UnsupportedError(
      'Browser.closeWindow is not available on this platform',
    );
  }

  @override
  void replaceAddress(String hashPath) {
    throw UnsupportedError(
      'Browser.replaceAddress is not available on this platform',
    );
  }

  @override
  void saveFile(List<int> bytes, String fileName, String mimeType) {
    throw UnsupportedError(
      'Browser.saveFile is not available on this platform',
    );
  }
}
