import '../../copy.dart';
import '../../routes.dart';

class SettingsEntry {
  const SettingsEntry({
    required this.title,
    required this.route,
    this.adminOnly = false,
  });

  final String title;
  final String route;
  final bool adminOnly;
}

/// Display order follows S9 7. Later tasks add History, Rules, Stats, Admin,
/// Experiments and the Bake-off report.
final List<SettingsEntry> settingsEntries = [
  const SettingsEntry(
    title: Copy.settingsConnectedAccounts,
    route: Routes.settingsAccounts,
  ),
  const SettingsEntry(title: Copy.history, route: Routes.settingsHistory),
  const SettingsEntry(title: Copy.rules, route: Routes.settingsRules),
  const SettingsEntry(title: Copy.stats, route: Routes.settingsStats),
  const SettingsEntry(
    title: Copy.settingsAccount,
    route: Routes.settingsAccount,
  ),
];
