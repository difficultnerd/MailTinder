import 'package:flutter/material.dart';

import '../../consent_text.dart';
import '../../copy.dart';
import '../../state/experiments_model.dart';
import '../../state/filed_model.dart' show LoadState;
import 'app_scope.dart';

/// S9 7.8: one off-by-default switch with the exact consent text.
class ExperimentsScreen extends StatefulWidget {
  const ExperimentsScreen({super.key});

  @override
  State<ExperimentsScreen> createState() => _ExperimentsScreenState();
}

class _ExperimentsScreenState extends State<ExperimentsScreen> {
  ExperimentsModel? _model;

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    _model ??= ExperimentsModel(api: AppScope.maybeOf(context)!.api)..load();
  }

  @override
  void dispose() {
    _model?.dispose();
    super.dispose();
  }

  Future<void> _toggle(bool on) async {
    final m = _model!;
    if (on) {
      await m.turnOn();
      return;
    }
    final ok = await showDialog<bool>(
      context: context,
      builder: (dialogContext) => AlertDialog(
        content: const Text(Copy.experimentsOffQuestion),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(dialogContext).pop(false),
            child: const Text(Copy.cancel),
          ),
          TextButton(
            onPressed: () => Navigator.of(dialogContext).pop(true),
            child: const Text(Copy.turnOff),
          ),
        ],
      ),
    );
    if (ok == true) {
      await m.turnOff();
    }
  }

  @override
  Widget build(BuildContext context) {
    final m = _model!;
    return Scaffold(
      appBar: AppBar(title: const Text(Copy.experiments)),
      body: ListenableBuilder(
        listenable: m,
        builder: (context, _) {
          if (m.state == LoadState.failed) {
            return Center(
              child: Column(
                mainAxisSize: MainAxisSize.min,
                children: [
                  const Text(Copy.genericError),
                  TextButton(
                    onPressed: m.load,
                    child: const Text(Copy.tryAgain),
                  ),
                ],
              ),
            );
          }
          if (m.state == LoadState.loading || m.mine == null) {
            return const Center(child: CircularProgressIndicator());
          }
          // Turning off always works; only turning on needs the experiment.
          final canChange = !m.busy && (m.isOn || !m.paused);
          return ListView(
            padding: const EdgeInsets.all(16),
            children: [
              const Text(kExperimentsConsentText),
              SwitchListTile(
                title: const Text(Copy.experimentsSwitch),
                value: m.isOn,
                onChanged: canChange ? _toggle : null,
              ),
              if (m.paused) const Text(Copy.experimentPaused),
              if (m.notice == ExperimentsNotice.consentChanged)
                const Text(Copy.consentChanged),
              if (m.notice == ExperimentsNotice.failed)
                const Text(Copy.actionFailed),
            ],
          );
        },
      ),
    );
  }
}
