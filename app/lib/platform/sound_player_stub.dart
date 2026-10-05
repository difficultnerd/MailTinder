import 'sound_player.dart';

/// Non-web [SoundPlayer]: silent. Keeps `flutter test` on the VM compiling.
class SoundPlayerImpl implements SoundPlayer {
  const SoundPlayerImpl();

  @override
  void play(SwipeSound s) {}
}
