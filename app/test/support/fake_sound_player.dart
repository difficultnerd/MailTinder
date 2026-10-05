import 'package:app/platform/sound_player.dart';

/// Records the sounds played.
class FakeSoundPlayer implements SoundPlayer {
  final List<SwipeSound> played = [];

  @override
  void play(SwipeSound s) => played.add(s);
}
