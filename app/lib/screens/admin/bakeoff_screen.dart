import 'package:flutter/material.dart';

import '../../api/api_client.dart';
import '../../api/models/bakeoff.dart';
import '../../copy.dart';
import '../../format.dart';
import '../../state/bakeoff_model.dart';
import '../settings/app_scope.dart';
import 'bakeoff_tables.dart';

/// S9 7.7: the bake-off report as plain tables, with filters, kill switches
/// and snapshots. Kill switches, saving and deleting go through Confirm it's
/// you; the server enforces `admin`, the hide is cosmetic.
class BakeoffScreen extends StatelessWidget {
  const BakeoffScreen({super.key});

  @override
  Widget build(BuildContext context) {
    final scope = AppScope.maybeOf(context)!;
    return ListenableBuilder(
      listenable: scope.session,
      builder: (context, _) {
        final isAdmin = scope.session.session?.user?.isAdmin ?? false;
        if (!isAdmin) {
          return Scaffold(
            appBar: AppBar(title: const Text(Copy.bakeoffReport)),
            body: const Center(child: Text(Copy.adminsOnly)),
          );
        }
        return const _BakeoffBody();
      },
    );
  }
}

class _BakeoffBody extends StatefulWidget {
  const _BakeoffBody();

  @override
  State<_BakeoffBody> createState() => _BakeoffBodyState();
}

class _BakeoffBodyState extends State<_BakeoffBody> {
  BakeoffModel? _model;
  final _name = TextEditingController();
  String? _nameError;

  AppScope get scope => AppScope.maybeOf(context)!;

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    _model ??= BakeoffModel(api: scope.api)..load();
  }

  @override
  void dispose() {
    _model?.dispose();
    _name.dispose();
    super.dispose();
  }

  void _toast(String text) {
    ScaffoldMessenger.of(context).showSnackBar(SnackBar(content: Text(text)));
  }

  /// Runs a write behind Confirm it's you. Returns null on cancel or error
  /// (the error toast is shown here).
  Future<T?> _write<T>(String label, Future<T> Function() action) async {
    try {
      return await scope.stepUp.run<T>(
        waitingActionLabel: label,
        action: action,
      );
    } on ApiException catch (e) {
      if (!mounted) {
        return null;
      }
      if (e.status == 409 && e.code == 'snapshot_limit') {
        _toast(Copy.deleteSnapshotFirst);
      } else if (e.status == 409 && e.code == 'versions_mixed') {
        _model!.enterVersionsMixed(e);
      } else {
        _toast(Copy.actionFailed);
      }
      return null;
    } on Object {
      if (mounted) {
        _toast(Copy.actionFailed);
      }
      return null;
    }
  }

  Future<void> _setSwitch(String model, bool on) async {
    final result = await _write<ClassifierExperiment>(
      Copy.stepUpKillSwitch(model, on),
      () => scope.api.setModelEnabled(model, on),
    );
    if (result != null && mounted) {
      _model!.applySwitches(result);
    }
  }

  Future<void> _save() async {
    final name = _name.text.trim();
    if (name.isEmpty || name.length > 100) {
      setState(() => _nameError = Copy.bakeoffSnapshotNameInvalid);
      return;
    }
    setState(() => _nameError = null);
    final model = _model!;
    final saved = await _write<Snapshot>(
      Copy.stepUpSaveSnapshot,
      () => scope.api.createSnapshot(name, model.query),
    );
    if (saved == null || !mounted) {
      return;
    }
    _name.clear();
    model.markSaved(name);
    await model.refreshSnapshots();
  }

  Future<void> _delete(SnapshotSummary s) async {
    final ok = await showDialog<bool>(
      context: context,
      builder: (dialogContext) => AlertDialog(
        content: Text(Copy.deleteSnapshotQuestion(s.name)),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(dialogContext).pop(false),
            child: const Text(Copy.cancel),
          ),
          TextButton(
            onPressed: () => Navigator.of(dialogContext).pop(true),
            child: const Text(Copy.delete),
          ),
        ],
      ),
    );
    if (ok != true || !mounted) {
      return;
    }
    final done = await _write<bool>(Copy.stepUpDeleteSnapshot, () async {
      await scope.api.deleteSnapshot(s.snapshotId);
      return true;
    });
    if (done == true && mounted) {
      await _model!.refreshSnapshots();
    }
  }

  Future<void> _download(
    Future<List<int>> Function() fetch,
    String fileName,
  ) async {
    try {
      final bytes = await fetch();
      scope.browser.saveFile(bytes, fileName, 'text/csv');
    } on Object {
      if (mounted) {
        _toast(Copy.actionFailed);
      }
    }
  }

  Future<void> _pickDate({required bool from}) async {
    final q = _model!.query;
    final current = from ? q.from : q.to;
    final picked = await showDatePicker(
      context: context,
      initialDate: DateTime(current.year, current.month, current.day),
      firstDate: DateTime(2026),
      lastDate: DateTime(2100),
    );
    if (picked == null || !mounted) {
      return;
    }
    final day = DateTime.utc(picked.year, picked.month, picked.day);
    _model!.query = from ? q.copyWith(from: day) : q.copyWith(to: day);
  }

  void _openSnapshot(SnapshotSummary s) {
    Navigator.of(context).push(
      MaterialPageRoute<void>(builder: (_) => _SnapshotScreen(summary: s)),
    );
  }

  @override
  Widget build(BuildContext context) {
    final model = _model!;
    return Scaffold(
      appBar: AppBar(title: const Text(Copy.bakeoffReport)),
      body: ListenableBuilder(
        listenable: model,
        builder: (context, _) => ListView(
          padding: const EdgeInsets.all(16),
          children: [
            _switches(model),
            const SizedBox(height: 16),
            _filters(model),
            const SizedBox(height: 16),
            _reportArea(model),
            const SizedBox(height: 16),
            _saveArea(model),
            const SizedBox(height: 16),
            _snapshotList(model),
          ],
        ),
      ),
    );
  }

  Widget _switches(BakeoffModel model) {
    final experiment = model.experiment;
    if (experiment == null) {
      return const SizedBox.shrink();
    }
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Semantics(
          header: true,
          child: Text(
            Copy.bakeoffKillSwitches,
            style: Theme.of(context).textTheme.titleMedium,
          ),
        ),
        for (final m in experiment.models)
          SwitchListTile(
            title: Text(Copy.modelLabel(m.model)),
            subtitle: Text(m.classifierId),
            value: m.enabled,
            onChanged: (on) => _setSwitch(m.model, on),
          ),
      ],
    );
  }

  Widget _versionDropdown({
    required String label,
    required List<String> options,
    required String? value,
    required void Function(String?) onChanged,
  }) {
    return SizedBox(
      width: 200,
      child: DropdownButtonFormField<String?>(
        decoration: InputDecoration(labelText: label),
        initialValue: options.contains(value) ? value : null,
        items: [
          const DropdownMenuItem<String?>(
            value: null,
            child: Text(Copy.bakeoffAnyVersion),
          ),
          for (final v in options)
            DropdownMenuItem<String?>(value: v, child: Text(v)),
        ],
        onChanged: onChanged,
      ),
    );
  }

  Widget _filters(BakeoffModel model) {
    final q = model.query;
    final present = model.versionsPresent;
    String day(DateTime d) => formatDate(DateTime(d.year, d.month, d.day));
    return Wrap(
      spacing: 12,
      runSpacing: 8,
      crossAxisAlignment: WrapCrossAlignment.center,
      children: [
        OutlinedButton(
          onPressed: () => _pickDate(from: true),
          child: Text('${Copy.bakeoffFrom} ${day(q.from)}'),
        ),
        OutlinedButton(
          onPressed: () => _pickDate(from: false),
          child: Text('${Copy.bakeoffTo} ${day(q.to)}'),
        ),
        _versionDropdown(
          label: Copy.bakeoffInputVersion,
          options: present?['input'] ?? const [],
          value: q.inputVersion,
          onChanged: (v) => model.query = v == null
              ? q.copyWith(clearInput: true)
              : q.copyWith(inputVersion: v),
        ),
        _versionDropdown(
          label: Copy.bakeoffQuestionVersion,
          options: present?['question'] ?? const [],
          value: q.questionVersion,
          onChanged: (v) => model.query = v == null
              ? q.copyWith(clearQuestion: true)
              : q.copyWith(questionVersion: v),
        ),
      ],
    );
  }

  Widget _reportArea(BakeoffModel model) {
    switch (model.state) {
      case BakeoffState.loading:
        return const Center(child: CircularProgressIndicator());
      case BakeoffState.failed:
        return Column(
          children: [
            const Text(Copy.actionFailed),
            TextButton(
              onPressed: model.loadReport,
              child: const Text(Copy.tryAgain),
            ),
          ],
        );
      case BakeoffState.notEnoughData:
        return const Text(Copy.notEnoughData);
      case BakeoffState.versionsMixed:
        return const Text(Copy.pickOneVersion);
      case BakeoffState.ready:
        return Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Align(
              alignment: Alignment.centerLeft,
              child: OutlinedButton(
                onPressed: () => _download(
                  () => scope.api.getBakeoffCsv(model.query),
                  'bakeoff-report.csv',
                ),
                child: const Text(Copy.downloadCsv),
              ),
            ),
            const SizedBox(height: 8),
            BakeoffTables(report: model.report!),
          ],
        );
    }
  }

  Widget _saveArea(BakeoffModel model) {
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        TextField(
          controller: _name,
          maxLength: 100,
          decoration: InputDecoration(
            labelText: Copy.bakeoffSnapshotName,
            errorText: _nameError,
          ),
        ),
        Align(
          alignment: Alignment.centerLeft,
          child: FilledButton(
            onPressed: _save,
            child: const Text(Copy.saveSnapshot),
          ),
        ),
        if (model.lastSavedName != null) const Text(Copy.snapshotSaved),
      ],
    );
  }

  Widget _snapshotList(BakeoffModel model) {
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Semantics(
          header: true,
          child: Text(
            Copy.bakeoffSnapshots,
            style: Theme.of(context).textTheme.titleMedium,
          ),
        ),
        if (model.snapshots.isEmpty) const Text(Copy.bakeoffNoSnapshots),
        for (final s in model.snapshots)
          ListTile(
            title: Text(s.name),
            subtitle: Text(
              '${formatDate(s.createdAt)}, ${Copy.snapshotCards(s.labelledSwipes)}',
            ),
            onTap: () => _openSnapshot(s),
            trailing: Wrap(
              children: [
                TextButton(
                  onPressed: () => _download(
                    () => scope.api.getSnapshotCsv(s.snapshotId),
                    'bakeoff-snapshot.csv',
                  ),
                  child: const Text(Copy.downloadCsv),
                ),
                TextButton(
                  onPressed: () => _delete(s),
                  child: const Text(Copy.delete),
                ),
              ],
            ),
          ),
      ],
    );
  }
}

/// A saved snapshot's report, read-only (same tables).
class _SnapshotScreen extends StatefulWidget {
  const _SnapshotScreen({required this.summary});

  final SnapshotSummary summary;

  @override
  State<_SnapshotScreen> createState() => _SnapshotScreenState();
}

class _SnapshotScreenState extends State<_SnapshotScreen> {
  Future<Snapshot>? _future;

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    _future ??= AppScope.maybeOf(
      context,
    )!.api.getSnapshot(widget.summary.snapshotId);
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      appBar: AppBar(title: Text(widget.summary.name)),
      body: FutureBuilder<Snapshot>(
        future: _future,
        builder: (context, snap) {
          if (snap.hasError) {
            return const Center(child: Text(Copy.actionFailed));
          }
          final data = snap.data;
          if (data == null) {
            return const Center(child: CircularProgressIndicator());
          }
          return ListView(
            padding: const EdgeInsets.all(16),
            children: [BakeoffTables(report: data.report)],
          );
        },
      ),
    );
  }
}
