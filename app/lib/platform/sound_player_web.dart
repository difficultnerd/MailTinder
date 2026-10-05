import 'package:web/web.dart' as web;

import 'sound_player.dart';

/// Web [SoundPlayer]: a 120 ms oscillator tone per sound `[DEFAULT]`. The
/// `AudioContext` is created on the first play, which only happens after a
/// swipe gesture, so browsers do not block it.
class SoundPlayerImpl implements SoundPlayer {
  const SoundPlayerImpl();

  /// One shared AudioContext for the page. Browsers cap how many may exist, so
  /// a static keeps this at one — and being static it does not stop the
  /// constructor from being `const`, which the analyzer requires on the VM
  /// build (where the stub is used) and the web build both.
  static web.AudioContext? _context;

  static const Map<SwipeSound, double> _hertz = {
    SwipeSound.keep: 660,
    SwipeSound.skip: 440,
    SwipeSound.reject: 220,
    SwipeSound.file: 880,
    SwipeSound.flush: 110,
  };

  @override
  void play(SwipeSound s) {
    try {
      final context = _context ??= web.AudioContext();
      final osc = context.createOscillator();
      osc.frequency.value = _hertz[s] ?? 440;
      osc.connect(context.destination);
      osc.start();
      osc.stop(context.currentTime + 0.12);
    } on Object {
      // Sound is a nicety; a blocked or missing AudioContext is ignored.
    }
  }
}
