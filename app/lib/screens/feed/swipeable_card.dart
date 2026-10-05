import 'package:flutter/material.dart';

import '../../api/models/feed.dart';
import '../../api/models/swipe.dart';
import '../../state/feedback_model.dart';
import 'effects.dart';

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
    this.feedback,
    this.card,
  });

  final Widget child;
  final VoidCallback onKeep;
  final VoidCallback onReject;
  final VoidCallback onFile;
  final VoidCallback onSkip;

  /// Plays the per-direction effect, sound and haptic for [card] (GM-02).
  final FeedbackModel? feedback;
  final FeedCard? card;

  @override
  State<SwipeableCard> createState() => SwipeableCardState();
}

/// Minimum drag distance (logical px) for a swipe to commit.
const double kSwipeDistance = 100;

/// Minimum release velocity (px/s) for a swipe to commit.
const double kSwipeVelocity = 800;

/// Fly-off animation duration.
const Duration kFlyOffDuration = Duration(milliseconds: 250);

/// How far a committed card travels as it flies off.
const double kFlyOffDistance = kEffectFlyDistance;

class SwipeableCardState extends State<SwipeableCard>
    with SingleTickerProviderStateMixin {
  late final AnimationController _fly = AnimationController(
    vsync: this,
    duration: kFlyOffDuration,
  )..addStatusListener(_onFlyStatus);

  Offset _drag = Offset.zero;
  Offset _flyFrom = Offset.zero;
  CardEffect _effect = CardEffect.flyRight;
  _Axis? _flyAxis;
  bool _flying = false;

  @override
  void didUpdateWidget(SwipeableCard oldWidget) {
    super.didUpdateWidget(oldWidget);
    // A new card takes the slot: forget the previous card's fly-off.
    if (oldWidget.card?.messageId != widget.card?.messageId) {
      _fly.stop();
      _flying = false;
      _flyAxis = null;
      _drag = Offset.zero;
    }
  }

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

    _play(axis);
  }

  /// Runs the same effects and fly-off a drag would, for the Keep, Reject,
  /// File and Skip buttons (GM-02 applies to every swipe however triggered).
  void swipe(SwipeKind kind) {
    if (_flying) return;
    _play(switch (kind) {
      SwipeKind.keep => _Axis.right,
      SwipeKind.reject => _Axis.left,
      SwipeKind.file => _Axis.up,
      SwipeKind.skip => _Axis.down,
    });
  }

  void _play(_Axis axis) {
    final kind = switch (axis) {
      _Axis.right => SwipeKind.keep,
      _Axis.left => SwipeKind.reject,
      _Axis.up => SwipeKind.file,
      _Axis.down => SwipeKind.skip,
    };
    final card = widget.card;
    final played = card == null ? null : widget.feedback?.onSwipe(card, kind);

    if (MediaQuery.disableAnimationsOf(context)) {
      _drag = Offset.zero;
      _commit(axis);
      return;
    }

    _effect =
        played ??
        switch (axis) {
          _Axis.right => CardEffect.flyRight,
          _Axis.left => CardEffect.flyLeft,
          _Axis.up => CardEffect.flyUp,
          _Axis.down => CardEffect.flyDown,
        };
    _fly.duration = effectDuration(_effect);
    _flyFrom = _drag;
    _flyAxis = axis;
    setState(() {
      _flying = true;
    });
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
        builder: (context, _) => applyCardEffect(
          effect: _effect,
          t: _fly.value,
          from: _flyFrom,
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
