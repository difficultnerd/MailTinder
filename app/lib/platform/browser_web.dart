import 'package:web/web.dart' as web;

import 'browser.dart';

/// Web implementation of [Browser] using `package:web`.
///
/// Imported only from `browser.dart` behind the conditional export, because
/// `package:web` does not compile on the VM.
class BrowserImpl implements Browser {
  const BrowserImpl();

  @override
  void assign(Uri url) {
    web.window.location.assign(url.toString());
  }

  @override
  void openExternal(Uri url) {
    web.window.open(url.toString(), '_blank', 'noopener,noreferrer');
  }

  @override
  BrowserPopup? openPopup(String name) {
    final popup = web.window.open(
      'about:blank',
      name,
      'popup,width=500,height=650',
    );
    if (popup == null) {
      return null;
    }
    return _WebBrowserPopup(popup);
  }

  @override
  bool get isPopupWindow => web.window.name == 'mt_step_up';

  @override
  void closeWindow() {
    web.window.close();
  }

  @override
  void replaceAddress(String hashPath) {
    web.window.history.replaceState(null, '', '#$hashPath');
  }
}

class _WebBrowserPopup implements BrowserPopup {
  _WebBrowserPopup(this._window);

  final web.Window _window;

  @override
  void navigate(Uri url) {
    _window.location.assign(url.toString());
  }

  @override
  void close() {
    _window.close();
  }
}
