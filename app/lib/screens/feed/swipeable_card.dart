import 'package:flutter/material.dart';

/// Wraps the focused card and turns a drag into a swipe (S9 section 3, S2
/// SW-01 to SW-04). The card follows the finger; on release the axis wins when
/// the distance is at least [kSwipeDistance] or the velocity at least
/// [kSwipeVelocity], otherwise it springs back: right keep, left reject, up
/// file, down skip. A committed swipe flies off over [kFlyOffDuration] and
/// then calls the matching callback.
///
/// The fly-off is driven by an [AnimationController] so it schedules frames and
/// settles in tests. Reduced motion (`MediaQuery.disableAnimationsOf`) commits
/// immediately, with no fly-off at all.
///
/// Horizontal and vertical drags use separate recognizers rather than one pan
/// recognizer: the card's vertical recognizer sits deeper in the hit-test tree
/// than the Feed's scrollable, so a downward drag on the card wins the gesture
/// arena and skips the card instead of scrolling the Feed.
class SwipeableCard extends StatefulWidget {
  const SwipeableCard({
    super.key,
    required this.child,
    required this.onKeep,
    required this.onReject,
    required this.onFile,
    required this.onSkip,
  });

  final Widget child;
  final VoidCallback onKeep;
  final VoidCallback onReject;
  final VoidCallback onFile;
  final VoidCallback onSkip;

  @override
  State<SwipeableCard> createState() => _SwipeableCardState();
}

/// Minimum drag distance (logical px) for a swipe to commit.
const double kSwipeDistance = 100;

/// Minimum release velocity (px/s) for a swipe to commit.
const double kSwipeVelocity = 800;

/// Fly-off animation duration.
const Duration kFlyOffDuration = Duration(milliseconds: 250);

/// How far a committed card travels as it flies off.
const double kFlyOffDistance = 1000;

class _SwipeableCardState extends State<SwipeableCard>
    with SingleTickerProviderStateMixin {
  late final AnimationController _fly = AnimationController(
    vsync: this,
    duration: kFlyOffDuration,
  )..addStatusListener(_onFlyStatus);

  Offset _drag = Offset.zero;
  Offset _flyFrom = Offset.zero;
  Offset _flyTo = Offset.zero;
  _Axis? _flyAxis;
  bool _flying = false;

  @override
  void dispose() {
    _fly.dispose();
    super.dispose();
  }

  void _onFlyStatus(AnimationStatus status) {
    final axis = _flyAxis;
    if (status == AnimationStatus.completed && _flying && axis != null) {
      _commit(axis);
    }
  }

  void _onDragUpdate(DragUpdateDetails details) {
    if (_flying) return;
    setState(() {
      _drag += details.delta;
    });
  }

  void _onHorizontalDragEnd(DragEndDetails details) {
    final axis = _drag.dx >= 0 ? _Axis.right : _Axis.left;
    _endDrag(axis, _drag.dx.abs(), details.primaryVelocity ?? 0);
  }

  void _onVerticalDragEnd(DragEndDetails details) {
    final axis = _drag.dy >= 0 ? _Axis.down : _Axis.up;
    _endDrag(axis, _drag.dy.abs(), details.primaryVelocity ?? 0);
  }

  void _endDrag(_Axis axis, double distance, double velocity) {
    if (_flying) return;
    if (distance < kSwipeDistance && velocity.abs() < kSwipeVelocity) {
      setState(() {
        _drag = Offset.zero;
      });
      return;
    }

    if (MediaQuery.disableAnimationsOf(context)) {
      _drag = Offset.zero;
      _commit(axis);
      return;
    }

    _flyFrom = _drag;
    _flyTo = switch (axis) {
      _Axis.right => const Offset(kFlyOffDistance, 0),
      _Axis.left => const Offset(-kFlyOffDistance, 0),
      _Axis.up => const Offset(0, -kFlyOffDistance),
      _Axis.down => const Offset(0, kFlyOffDistance),
    };
    _flyAxis = axis;
    _flying = true;
    _fly.forward(from: 0);
  }

  void _commit(_Axis axis) {
    switch (axis) {
      case _Axis.right:
        widget.onKeep();
      case _Axis.left:
        widget.onReject();
      case _Axis.up:
        widget.onFile();
      case _Axis.down:
        widget.onSkip();
    }
  }

  @override
  Widget build(BuildContext context) {
    Widget child;
    if (_flying) {
      child = AnimatedBuilder(
        animation: _fly,
        builder: (context, _) => Transform.translate(
          offset: Offset.lerp(_flyFrom, _flyTo, _fly.value) ?? _flyTo,
          child: widget.child,
        ),
      );
    } else if (_drag != Offset.zero) {
      child = Transform.translate(offset: _drag, child: widget.child);
    } else {
      child = widget.child;
    }
    return GestureDetector(
      onHorizontalDragUpdate: _onDragUpdate,
      onHorizontalDragEnd: _onHorizontalDragEnd,
      onVerticalDragUpdate: _onDragUpdate,
      onVerticalDragEnd: _onVerticalDragEnd,
      child: child,
    );
  }
}

enum _Axis { right, left, up, down }
