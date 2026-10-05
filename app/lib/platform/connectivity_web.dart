import 'dart:async';
import 'dart:js_interop';

import 'package:web/web.dart' as web;

import 'connectivity.dart';

/// Web implementation of [Connectivity] using `navigator.onLine` and the
/// browser's `online`/`offline` events. Imported only from `connectivity.dart`
/// behind the conditional export, because `package:web` does not compile on
/// the VM.
class ConnectivityImpl implements Connectivity {
  ConnectivityImpl() {
    _listener = _handleChange.toJS;
    web.window.addEventListener('online', _listener);
    web.window.addEventListener('offline', _listener);
  }

  final StreamController<bool> _controller = StreamController<bool>.broadcast();
  late final JSFunction _listener;

  void _handleChange(web.Event _) {
    _controller.add(web.window.navigator.onLine);
  }

  @override
  bool get isOnline => web.window.navigator.onLine;

  @override
  Stream<bool> get changes => _controller.stream;

  void dispose() {
    web.window.removeEventListener('online', _listener);
    web.window.removeEventListener('offline', _listener);
    _controller.close();
  }
}
