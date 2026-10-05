import 'package:app/platform/haptics.dart';

/// Records taps; [supported] is set by the test.
class FakeHaptics implements Haptics {
  FakeHaptics({this.supported = true});

  @override
  final bool supported;

  int taps = 0;

  @override
  void tap() => taps++;
}
