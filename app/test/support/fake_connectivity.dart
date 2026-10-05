import 'dart:async';

import 'package:app/platform/connectivity.dart';

/// Test double for [Connectivity]:
/// - [online] controls [isOnline] (start with `online`)
/// - [setOnline] flips the flag and emits on [changes]
class FakeConnectivity implements Connectivity {
  FakeConnectivity({this.online = true});

  bool online;

  final StreamController<bool> _controller = StreamController<bool>.broadcast();

  @override
  bool get isOnline => online;

  @override
  Stream<bool> get changes => _controller.stream;

  void setOnline(bool value) {
    online = value;
    _controller.add(value);
  }

  void dispose() {
    _controller.close();
  }
}
