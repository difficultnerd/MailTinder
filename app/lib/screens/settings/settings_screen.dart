import 'package:flutter/material.dart';

import '../../copy.dart';
import 'app_scope.dart';
import 'settings_entries.dart';

class SettingsScreen extends StatelessWidget {
  const SettingsScreen({super.key});

  @override
  Widget build(BuildContext context) {
    final scope = AppScope.maybeOf(context);
    if (scope == null) {
      return const Center(child: Text(Copy.tabSettings));
    }
    return ListenableBuilder(
      listenable: scope.session,
      builder: (context, _) {
        final isAdmin = scope.session.session?.user?.isAdmin ?? false;
        final entries = settingsEntries
            .where((e) => !e.adminOnly || isAdmin)
            .toList(growable: false);
        return Scaffold(
          appBar: AppBar(title: const Text(Copy.tabSettings)),
          body: ListView(
            children: [
              for (final entry in entries)
                ListTile(
                  title: Text(entry.title),
                  trailing: const Icon(Icons.chevron_right),
                  onTap: () => Navigator.of(context).pushNamed(entry.route),
                ),
            ],
          ),
        );
      },
    );
  }
}
