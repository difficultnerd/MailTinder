/// Connectivity (online/offline) without browser storage.
///
/// The web implementation lives in `connectivity_web.dart` and uses
/// `package:web` plus `navigator.onLine` and the `online`/`offline` events.
/// `package:web` does not compile on the VM, so it is imported only behind
/// the conditional export below; the non-web build uses
/// `connectivity_stub.dart`, which keeps `flutter test` on the VM compiling.
library;

export 'connectivity_stub.dart'
    if (dart.library.js_interop) 'connectivity_web.dart';

/// Whether the app is online and a stream of changes.
abstract class Connectivity {
  bool get isOnline;

  Stream<bool> get changes;
}
