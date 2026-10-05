import 'package:app/api/models/swipe.dart';
import 'package:app/platform/sound_player.dart';
import 'package:app/state/feedback_model.dart';
import 'package:app/state/play_prefs.dart';
import 'package:flutter_test/flutter_test.dart';

import '../support/cards.dart';
import '../support/fake_haptics.dart';
import '../support/fake_sound_player.dart';

void main() {
  late DateTime clock;
  late PlayPrefs prefs;
  late FakeSoundPlayer sound;
  late FakeHaptics haptics;

  FeedbackModel make({bool vibrate = true}) {
    clock = DateTime.utc(2026, 1, 1);
    prefs = PlayPrefs();
    sound = FakeSoundPlayer();
    haptics = FakeHaptics(supported: vibrate);
    return FeedbackModel(
      prefs: prefs,
      sound: sound,
      haptics: haptics,
      now: () => clock,
    );
  }

  test('GM-02 AC1 effect per direction and flush on a high score', () {
    final m = make();
    expect(m.onSwipe(buildCard(), SwipeKind.keep), CardEffect.flyRight);
    expect(m.onSwipe(buildCard(), SwipeKind.reject), CardEffect.flyLeft);
    expect(m.onSwipe(buildCard(), SwipeKind.file), CardEffect.flyUp);
    expect(m.onSwipe(buildCard(), SwipeKind.skip), CardEffect.flyDown);
    expect(
      m.onSwipe(buildCard(bulkScore: kFlushScore), SwipeKind.reject),
      CardEffect.flush,
    );
  });

  test('GM-02 AC2 haptics only when the Vibration API exists', () {
    var m = make(vibrate: false);
    m.onSwipe(buildCard(), SwipeKind.keep);
    expect(haptics.taps, 0);
    m = make();
    m.onSwipe(buildCard(), SwipeKind.keep);
    expect(haptics.taps, 1);
  });

  test('GM-02 AC2 sound plays only when Sounds is on', () {
    final m = make();
    m.onSwipe(buildCard(), SwipeKind.keep);
    expect(sound.played, isEmpty);
    prefs.soundsOn = true;
    m.onSwipe(buildCard(), SwipeKind.skip);
    expect(sound.played, [SwipeSound.skip]);
  });

  test('GM-02 AC3 combo appears after 5 swipes within 10 seconds', () {
    final m = make();
    for (var i = 0; i < kComboSwipes - 1; i++) {
      m.onSwipe(buildCard(), SwipeKind.keep);
      clock = clock.add(const Duration(seconds: 1));
    }
    expect(m.combo, isNull);
    m.onSwipe(buildCard(), SwipeKind.keep);
    expect(m.combo, kComboSwipes);
  });

  test('GM-02 AC3 no combo when swipes are spread out', () {
    final m = make();
    for (var i = 0; i < 10; i++) {
      m.onSwipe(buildCard(), SwipeKind.keep);
      clock = clock.add(const Duration(seconds: 4));
    }
    expect(m.combo, isNull);
  });

  test('GM-02 AC3 confetti is due once per 100 cleared', () {
    final m = make();
    for (var i = 0; i < kConfettiEvery - 1; i++) {
      m.onSwipe(buildCard(), i.isEven ? SwipeKind.reject : SwipeKind.file);
    }
    m.onSwipe(buildCard(), SwipeKind.keep);
    expect(m.confettiDue, isFalse);
    m.onSwipe(buildCard(), SwipeKind.reject);
    expect(m.confettiDue, isTrue);
    expect(m.milestone, kConfettiEvery);
    m.acknowledgeConfetti();
    expect(m.confettiDue, isFalse);
  });
}
