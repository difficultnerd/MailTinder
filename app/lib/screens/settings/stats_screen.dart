import 'package:flutter/material.dart';

import '../../achievements.dart';
import '../../api/models/stats.dart';
import '../../api/models/swipe.dart';
import '../../copy.dart';
import '../../format.dart';
import '../../state/filed_model.dart' show LoadState;
import '../../state/stats_model.dart';
import 'app_scope.dart';

/// S9 7.4: totals and the achievements list.
class StatsScreen extends StatefulWidget {
  const StatsScreen({super.key});

  @override
  State<StatsScreen> createState() => _StatsScreenState();
}

class _StatsScreenState extends State<StatsScreen> {
  StatsModel? _model;

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    _model ??= StatsModel(api: AppScope.maybeOf(context)!.api)..load();
  }

  @override
  void dispose() {
    _model?.dispose();
    super.dispose();
  }

  Widget _figure(String label, int n) =>
      ListTile(title: Text(label), trailing: Text(formatCount(n)));

  Widget _achievements(Stats s) {
    final unlocked = {for (final a in s.achievements) a.achievementId: a};
    final unknown = <Achievement>[
      for (final a in s.achievements)
        if (achievementTitle(a.achievementId) == null) a,
    ];
    return Column(
      children: [
        for (final info in kAchievements)
          if (unlocked[info.id] case final a?)
            ListTile(
              title: Text(info.title),
              subtitle: Text(formatDate(a.unlockedAt)),
            )
          else
            Opacity(
              opacity: 0.4,
              child: Semantics(
                label: Copy.achievementLocked(info.title),
                container: true,
                excludeSemantics: true,
                child: ListTile(title: Text(info.title)),
              ),
            ),
        for (final a in unknown)
          ListTile(
            title: const Text(Copy.achievementUnknown),
            subtitle: Text(formatDate(a.unlockedAt)),
          ),
      ],
    );
  }

  @override
  Widget build(BuildContext context) {
    final m = _model!;
    return Scaffold(
      appBar: AppBar(title: const Text(Copy.stats)),
      body: ListenableBuilder(
        listenable: m,
        builder: (context, _) {
          final s = m.stats;
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
          if (m.state == LoadState.loading || s == null) {
            return const Center(child: CircularProgressIndicator());
          }
          return ListView(
            children: [
              _figure(Copy.emailsTriaged, s.emailsTriaged),
              _figure(Copy.sendersUnsubscribed, s.sendersUnsubscribed),
              _figure(Copy.unsubscribesConfirmed, s.unsubscribesConfirmed),
              ListTile(
                title: Text(Copy.mailStoppedTotal(s.mailStoppedPerYear)),
              ),
              const Divider(),
              _achievements(s),
            ],
          );
        },
      ),
    );
  }
}
