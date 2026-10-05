import 'dart:async';
import 'dart:math' as math;

import 'package:flutter/material.dart';

import '../../copy.dart';
import '../../state/feedback_model.dart';

/// Fly-off duration for the four directions.
const Duration kEffectFlyDuration = Duration(milliseconds: 250);

/// Flush duration `[DEFAULT]`: a 360 degree spin while shrinking to nothing.
const Duration kFlushDuration = Duration(milliseconds: 400);

/// Confetti duration `[DEFAULT]`.
const Duration kConfettiDuration = Duration(milliseconds: 1500);

/// How long the reduced-motion milestone text shows.
const Duration kMilestoneTextDuration = Duration(seconds: 2);

const int kConfettiParticles = 60;

/// How far a card travels in a fly-off.
const double kEffectFlyDistance = 1000;

Duration effectDuration(CardEffect e) =>
    e == CardEffect.flush ? kFlushDuration : kEffectFlyDuration;

/// Applies [effect] to [child] at animation value [t] (0 to 1), starting from
/// the drag offset [from].
Widget applyCardEffect({
  required CardEffect effect,
  required double t,
  required Offset from,
  required Widget child,
}) {
  final keyed = KeyedSubtree(
    key: ValueKey('effect-${effect.name}'),
    child: child,
  );
  if (effect == CardEffect.flush) {
    return Transform.translate(
      offset: from,
      child: Transform.rotate(
        angle: 2 * math.pi * t,
        child: Transform.scale(scale: 1 - t, child: keyed),
      ),
    );
  }
  final to = switch (effect) {
    CardEffect.flyRight => const Offset(kEffectFlyDistance, 0),
    CardEffect.flyLeft => const Offset(-kEffectFlyDistance, 0),
    CardEffect.flyUp => const Offset(0, -kEffectFlyDistance),
    _ => const Offset(0, kEffectFlyDistance),
  };
  return Transform.translate(
    offset: Offset.lerp(from, to, t) ?? to,
    child: keyed,
  );
}

/// The small "Combo n" badge.
class ComboBadge extends StatelessWidget {
  const ComboBadge({super.key, required this.count});

  final int count;

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    return Semantics(
      label: Copy.combo(count),
      child: Material(
        color: scheme.tertiaryContainer,
        borderRadius: BorderRadius.circular(12),
        child: Padding(
          padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 4),
          child: Text(
            Copy.combo(count),
            style: TextStyle(color: scheme.onTertiaryContainer),
          ),
        ),
      ),
    );
  }
}

/// Confetti for every [kConfettiEvery] cleared (GM-02 AC3). With reduced
/// motion it is a static line of text for [kMilestoneTextDuration] instead
/// (GM-02 AC4, XC-03). Calls [onDone] when finished.
class MilestoneOverlay extends StatefulWidget {
  const MilestoneOverlay({
    super.key,
    required this.milestone,
    required this.onDone,
  });

  final int milestone;
  final VoidCallback onDone;

  @override
  State<MilestoneOverlay> createState() => _MilestoneOverlayState();
}

class _MilestoneOverlayState extends State<MilestoneOverlay>
    with SingleTickerProviderStateMixin {
  late final AnimationController _controller = AnimationController(
    vsync: this,
    duration: kConfettiDuration,
  );
  Timer? _timer;
  bool _started = false;

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    if (_started) return;
    _started = true;
    if (MediaQuery.disableAnimationsOf(context)) {
      // A plain timer: no animation runs under reduced motion.
      _timer = Timer(kMilestoneTextDuration, widget.onDone);
      return;
    }
    _controller.forward().whenComplete(() {
      if (mounted) widget.onDone();
    });
  }

  @override
  void dispose() {
    _timer?.cancel();
    _controller.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    if (MediaQuery.disableAnimationsOf(context)) {
      return IgnorePointer(
        child: Align(
          alignment: Alignment.topCenter,
          child: Padding(
            padding: const EdgeInsets.only(top: 48),
            child: Semantics(
              liveRegion: true,
              child: Material(
                borderRadius: BorderRadius.circular(12),
                color: Theme.of(context).colorScheme.primaryContainer,
                child: Padding(
                  padding: const EdgeInsets.all(12),
                  child: Text(Copy.clearedMilestone(widget.milestone)),
                ),
              ),
            ),
          ),
        ),
      );
    }
    return IgnorePointer(
      child: AnimatedBuilder(
        animation: _controller,
        builder: (context, _) => CustomPaint(
          key: const ValueKey('confetti'),
          size: Size.infinite,
          painter: ConfettiPainter(_controller.value),
        ),
      ),
    );
  }
}

/// 60 particles falling over the animation, no package.
class ConfettiPainter extends CustomPainter {
  ConfettiPainter(this.t);

  final double t;

  static const _colors = [
    Colors.red,
    Colors.orange,
    Colors.green,
    Colors.blue,
    Colors.purple,
  ];

  @override
  void paint(Canvas canvas, Size size) {
    final rng = math.Random(7);
    for (var i = 0; i < kConfettiParticles; i++) {
      final x = rng.nextDouble() * size.width;
      final speed = 0.6 + rng.nextDouble() * 0.8;
      final drift = (rng.nextDouble() - 0.5) * 80;
      final y = -20 + t * speed * (size.height + 40);
      final paint = Paint()
        ..color = _colors[i % _colors.length].withValues(alpha: 1 - t * 0.6);
      canvas.save();
      canvas.translate(x + drift * t, y);
      canvas.rotate(t * 6 * (i.isEven ? 1 : -1));
      canvas.drawRect(const Rect.fromLTWH(-4, -2, 8, 4), paint);
      canvas.restore();
    }
  }

  @override
  bool shouldRepaint(ConfettiPainter old) => old.t != t;
}
