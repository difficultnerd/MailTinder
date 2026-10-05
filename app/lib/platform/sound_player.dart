/// Optional swipe sounds, without audio files or browser storage.
///
/// The web implementation lives in `sound_player_web.dart` and uses
/// `package:web` (WebAudio tones). `package:web` does not compile on the VM,
/// so it is imported only behind the conditional export below; the non-web
/// build uses `sound_player_stub.dart`.
library;

export 'sound_player_stub.dart'
    if (dart.library.js_interop) 'sound_player_web.dart';

/// One sound per swipe outcome (GM-02 AC2).
enum SwipeSound { keep, skip, reject, file, flush }

/// Plays a short sound. Callers check the Sounds switch first.
abstract class SoundPlayer {
  void play(SwipeSound s);
}
