import 'package:flutter/material.dart';

import '../../api/api_client.dart';
import '../../copy.dart';
import '../../platform/browser.dart';
import '../../state/needs_attention_model.dart';
import '../../state/session_model.dart';
import '../feed/feed_screen.dart';
import '../filed/filed_screen.dart';
import '../needs_attention/needs_attention_screen.dart';
import '../settings/settings_screen.dart';

/// The four bottom tabs (T-007). The Needs Attention tab carries a badge with
/// the open item count when one is available (S9 section 6, NA-02).
///
/// The optional dependencies are supplied by tests and later tasks; with none
/// of them the shell shows the placeholder screens.
class HomeShell extends StatefulWidget {
  const HomeShell({
    super.key,
    this.needsAttention,
    this.session,
    this.api,
    this.browser,
  });

  final NeedsAttentionModel? needsAttention;
  final SessionModel? session;
  final ApiClient? api;
  final Browser? browser;

  @override
  State<HomeShell> createState() => _HomeShellState();
}

class _HomeShellState extends State<HomeShell> {
  int _selectedIndex = 0;

  @override
  Widget build(BuildContext context) {
    final model = widget.needsAttention;
    return Scaffold(
      body: IndexedStack(
        index: _selectedIndex,
        children: [
          const FeedScreen(),
          const FiledScreen(),
          NeedsAttentionScreen(
            model: model,
            session: widget.session,
            api: widget.api,
            browser: widget.browser,
          ),
          const SettingsScreen(),
        ],
      ),
      bottomNavigationBar: model == null
          ? _navigationBar(0)
          : ListenableBuilder(
              listenable: model,
              builder: (context, _) => _navigationBar(model.openCount),
            ),
    );
  }

  NavigationBar _navigationBar(int openCount) {
    return NavigationBar(
      selectedIndex: _selectedIndex,
      onDestinationSelected: (index) {
        setState(() {
          _selectedIndex = index;
        });
      },
      destinations: [
        _destination(
          label: Copy.tabFeed,
          icon: Icons.inbox_outlined,
          selectedIcon: Icons.inbox,
        ),
        _destination(
          label: Copy.tabFiled,
          icon: Icons.folder_outlined,
          selectedIcon: Icons.folder,
        ),
        _destination(
          label: Copy.tabNeedsAttention,
          icon: Icons.warning_amber_outlined,
          selectedIcon: Icons.warning,
          badgeCount: openCount,
        ),
        _destination(
          label: Copy.tabSettings,
          icon: Icons.settings_outlined,
          selectedIcon: Icons.settings,
        ),
      ],
    );
  }

  NavigationDestination _destination({
    required String label,
    required IconData icon,
    required IconData selectedIcon,
    int badgeCount = 0,
  }) {
    return NavigationDestination(
      icon: _withBadge(badgeCount, Semantics(label: label, child: Icon(icon))),
      selectedIcon: _withBadge(
        badgeCount,
        Semantics(label: label, child: Icon(selectedIcon)),
      ),
      label: label,
    );
  }

  Widget _withBadge(int count, Widget child) {
    if (count <= 0) return child;
    return Badge(label: Text('$count'), child: child);
  }
}
