import 'haptics.dart';

/// Non-web [Haptics]: unsupported.
class HapticsImpl implements Haptics {
  const HapticsImpl();

  @override
  bool get supported => false;

  @override
  void tap() {}
}
