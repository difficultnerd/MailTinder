import 'package:flutter/material.dart';

import '../../copy.dart';
import '../feed/feed_screen.dart';
import '../filed/filed_screen.dart';
import '../needs_attention/needs_attention_screen.dart';
import '../settings/settings_screen.dart';

class HomeShell extends StatefulWidget {
  const HomeShell({super.key});

  @override
  State<HomeShell> createState() => _HomeShellState();
}

class _HomeShellState extends State<HomeShell> {
  int _selectedIndex = 0;

  static const List<Widget> _screens = [
    FeedScreen(),
    FiledScreen(),
    NeedsAttentionScreen(),
    SettingsScreen(),
  ];

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      body: IndexedStack(index: _selectedIndex, children: _screens),
      bottomNavigationBar: NavigationBar(
        selectedIndex: _selectedIndex,
        onDestinationSelected: (index) {
          setState(() {
            _selectedIndex = index;
          });
        },
        destinations: [
          NavigationDestination(
            icon: Semantics(
              label: Copy.tabFeed,
              child: const Icon(Icons.inbox_outlined),
            ),
            selectedIcon: Semantics(
              label: Copy.tabFeed,
              child: const Icon(Icons.inbox),
            ),
            label: Copy.tabFeed,
          ),
          NavigationDestination(
            icon: Semantics(
              label: Copy.tabFiled,
              child: const Icon(Icons.folder_outlined),
            ),
            selectedIcon: Semantics(
              label: Copy.tabFiled,
              child: const Icon(Icons.folder),
            ),
            label: Copy.tabFiled,
          ),
          NavigationDestination(
            icon: Semantics(
              label: Copy.tabNeedsAttention,
              child: const Icon(Icons.warning_amber_outlined),
            ),
            selectedIcon: Semantics(
              label: Copy.tabNeedsAttention,
              child: const Icon(Icons.warning),
            ),
            label: Copy.tabNeedsAttention,
          ),
          NavigationDestination(
            icon: Semantics(
              label: Copy.tabSettings,
              child: const Icon(Icons.settings_outlined),
            ),
            selectedIcon: Semantics(
              label: Copy.tabSettings,
              child: const Icon(Icons.settings),
            ),
            label: Copy.tabSettings,
          ),
        ],
      ),
    );
  }
}
