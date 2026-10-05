import 'package:app/platform/browser.dart';

/// Records every [Browser] call for assertions in widget tests.
class FakeBrowser implements Browser {
  final List<Uri> assigned = [];
  final List<Uri> openedExternal = [];
  final List<String> openedPopups = [];
  final List<String> replacedAddresses = [];
  bool popupBlocked = false;
  bool popupWindow = false;
  bool closedWindow = false;

  @override
  void assign(Uri url) {
    assigned.add(url);
  }

  @override
  void openExternal(Uri url) {
    openedExternal.add(url);
  }

  @override
  BrowserPopup? openPopup(String name) {
    openedPopups.add(name);
    if (popupBlocked) {
      return null;
    }
    return FakeBrowserPopup();
  }

  @override
  bool get isPopupWindow => popupWindow;

  @override
  void closeWindow() {
    closedWindow = true;
  }

  @override
  void replaceAddress(String hashPath) {
    replacedAddresses.add(hashPath);
  }
}

class FakeBrowserPopup implements BrowserPopup {
  final List<Uri> navigated = [];
  bool closed = false;

  @override
  void navigate(Uri url) {
    navigated.add(url);
  }

  @override
  void close() {
    closed = true;
  }
}
