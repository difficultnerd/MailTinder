import 'connectivity.dart';

/// Non-web implementation of [Connectivity] that reports online and never
/// changes. Keeps `flutter test` on the VM compiling; the real behaviour is in
/// `connectivity_web.dart`, which is selected by the conditional export in
/// `connectivity.dart`.
class ConnectivityImpl implements Connectivity {
  const ConnectivityImpl();

  @override
  bool get isOnline => true;

  @override
  Stream<bool> get changes => const Stream.empty();
}
