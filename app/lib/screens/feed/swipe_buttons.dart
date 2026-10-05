import 'package:flutter/material.dart';

import '../../copy.dart';
import '../../state/swipe_controller.dart';

/// The Feed action buttons (S9 section 3): Reject, Skip, File, Keep and Undo.
/// Each button calls the same controller method as its gesture (gesture
/// parity, S10 3.2) and carries a Semantics label equal to its name. Swipe
/// buttons are disabled while the controller is disabled (offline included);
/// Undo is disabled when the undo stack is empty.
class SwipeButtons extends StatelessWidget {
  const SwipeButtons({super.key, required this.controller});

  final SwipeController controller;

  @override
  Widget build(BuildContext context) {
    return Row(
      mainAxisAlignment: MainAxisAlignment.spaceEvenly,
      children: [
        _button(
          label: Copy.rejectButton,
          icon: Icons.close,
          onPressed: controller.enabled ? controller.reject : null,
        ),
        _button(
          label: Copy.skipButton,
          icon: Icons.arrow_downward,
          onPressed: controller.enabled ? controller.skip : null,
        ),
        _button(
          label: Copy.fileButton,
          icon: Icons.folder_open,
          onPressed: controller.enabled ? () => controller.file(context) : null,
        ),
        _button(
          label: Copy.keepButton,
          icon: Icons.check,
          onPressed: controller.enabled ? controller.keep : null,
        ),
        _button(
          label: Copy.undoButton,
          icon: Icons.undo,
          onPressed: controller.canUndo ? controller.undo : null,
        ),
      ],
    );
  }

  Widget _button({
    required String label,
    required IconData icon,
    required VoidCallback? onPressed,
  }) {
    return Semantics(
      label: label,
      button: true,
      child: IconButton(onPressed: onPressed, icon: Icon(icon), tooltip: label),
    );
  }
}
