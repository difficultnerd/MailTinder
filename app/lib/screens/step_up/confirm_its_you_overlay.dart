import 'package:flutter/material.dart';

import '../../copy.dart';
import '../../state/step_up_controller.dart';

/// The "Confirm it's you" panel shown over the current screen (S9 section
/// 1.1). The host wraps every screen; this panel appears while a step-up is in
/// progress.
class ConfirmItsYouOverlay extends StatelessWidget {
  const ConfirmItsYouOverlay({super.key, required this.controller});

  final StepUpController controller;

  @override
  Widget build(BuildContext context) {
    final status = controller.status;
    final redirecting = status == StepUpStatus.redirecting;
    final blocked = status == StepUpStatus.popupBlocked;
    final label = controller.waitingActionLabel;

    return Material(
      elevation: 8,
      borderRadius: BorderRadius.circular(12),
      child: ConstrainedBox(
        constraints: const BoxConstraints(maxWidth: 360),
        child: Padding(
          padding: const EdgeInsets.all(24),
          child: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              Text(
                Copy.confirmItsYou,
                textAlign: TextAlign.center,
                style: Theme.of(
                  context,
                ).textTheme.titleLarge?.copyWith(fontWeight: FontWeight.bold),
              ),
              if (label != null) ...[
                const SizedBox(height: 8),
                Text(label, textAlign: TextAlign.center),
              ],
              const SizedBox(height: 8),
              const Text(Copy.stepUpExplainer, textAlign: TextAlign.center),
              if (blocked) ...[
                const SizedBox(height: 12),
                const Text(Copy.popupBlocked, textAlign: TextAlign.center),
              ],
              const SizedBox(height: 16),
              Semantics(
                label: Copy.continueWithGoogle,
                button: true,
                child: ElevatedButton(
                  onPressed: redirecting ? null : controller.continueWithGoogle,
                  child: redirecting
                      ? const SizedBox(
                          width: 20,
                          height: 20,
                          child: CircularProgressIndicator(strokeWidth: 2),
                        )
                      : const Text(Copy.continueWithGoogle),
                ),
              ),
              const SizedBox(height: 4),
              Semantics(
                label: Copy.cancel,
                button: true,
                child: TextButton(
                  onPressed: controller.cancel,
                  child: const Text(Copy.cancel),
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }
}

/// Wraps the whole app and hosts the step-up overlay plus the "Not confirmed"
/// SnackBar. Used through `MaterialApp.builder`.
class StepUpOverlayHost extends StatefulWidget {
  const StepUpOverlayHost({
    super.key,
    required this.controller,
    required this.child,
  });

  final StepUpController controller;
  final Widget child;

  @override
  State<StepUpOverlayHost> createState() => _StepUpOverlayHostState();
}

class _StepUpOverlayHostState extends State<StepUpOverlayHost> {
  final GlobalKey<ScaffoldMessengerState> _messengerKey =
      GlobalKey<ScaffoldMessengerState>();
  int _seenNotConfirmed = 0;

  @override
  void initState() {
    super.initState();
    _seenNotConfirmed = widget.controller.notConfirmedCount;
    widget.controller.addListener(_handleController);
  }

  @override
  void dispose() {
    widget.controller.removeListener(_handleController);
    super.dispose();
  }

  void _handleController() {
    final count = widget.controller.notConfirmedCount;
    if (widget.controller.status == StepUpStatus.idle &&
        count != _seenNotConfirmed) {
      _seenNotConfirmed = count;
      _messengerKey.currentState?.showSnackBar(
        const SnackBar(content: Text(Copy.stepUpNotConfirmed)),
      );
    }
  }

  @override
  Widget build(BuildContext context) {
    return ScaffoldMessenger(
      key: _messengerKey,
      child: Stack(
        children: [
          widget.child,
          ListenableBuilder(
            listenable: widget.controller,
            builder: (context, _) {
              if (widget.controller.status == StepUpStatus.idle) {
                return const SizedBox.shrink();
              }
              return Positioned.fill(
                child: Stack(
                  children: [
                    const ModalBarrier(
                      dismissible: false,
                      color: Colors.black54,
                    ),
                    Center(
                      child: ConfirmItsYouOverlay(
                        controller: widget.controller,
                      ),
                    ),
                  ],
                ),
              );
            },
          ),
        ],
      ),
    );
  }
}
