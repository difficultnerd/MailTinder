import 'package:flutter/material.dart';

import '../../api/api_client.dart';
import '../../copy.dart';
import '../../platform/browser.dart';
import '../../platform/connectivity.dart';
import '../../state/feed_model.dart';
import '../../state/filed_model.dart';
import '../../state/progress_model.dart';
import '../../state/round_tracker.dart';
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
    this.feed,
    this.filed,
    this.progress,
    this.rounds,
  });

  final NeedsAttentionModel? needsAttention;
  final SessionModel? session;
  final ApiClient? api;
  final Browser? browser;

  /// Live models. Each is built from [api], [session] and [browser] when absent
  /// and those are supplied; without them the tabs are placeholders.
  final FeedModel? feed;
  final FiledModel? filed;
  final ProgressModel? progress;
  final RoundTracker? rounds;

  @override
  State<HomeShell> createState() => _HomeShellState();
}

class _HomeShellState extends State<HomeShell> {
  int _selectedIndex = 0;
  final ValueNotifier<bool> _feedVisible = ValueNotifier<bool>(true);
  FeedModel? _feed;
  FiledModel? _filed;
  ProgressModel? _progress;
  RoundTracker? _rounds;
  NeedsAttentionModel? _needsAttention;

  @override
  void initState() {
    super.initState();
    final api = widget.api;
    final session = widget.session;
    if (api != null && session != null && widget.browser != null) {
      _feed =
          widget.feed ??
          FeedModel(
            api: api,
            session: session,
            // The web implementation has no const constructor.
            // ignore: prefer_const_constructors
            connectivity: ConnectivityImpl(),
          );
      _filed = widget.filed ?? FiledModel(api: api);
      _progress = widget.progress ?? ProgressModel(api: api);
      _rounds = widget.rounds ?? RoundTracker();
      _needsAttention = widget.needsAttention ?? NeedsAttentionModel(api: api);
    } else {
      _needsAttention = widget.needsAttention;
    }
  }

  @override
  void dispose() {
    _feedVisible.dispose();
    if (widget.feed == null) _feed?.dispose();
    if (widget.filed == null) _filed?.dispose();
    if (widget.progress == null) _progress?.dispose();
    if (widget.rounds == null) _rounds?.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final model = _needsAttention;
    return Scaffold(
      body: IndexedStack(
        index: _selectedIndex,
        children: [
          FeedScreen(
            model: _feed,
            session: widget.session,
            api: widget.api,
            browser: widget.browser,
            progress: _progress,
            rounds: _rounds,
            feedVisible: _feedVisible,
          ),
          FiledScreen(
            model: _filed,
            session: widget.session,
            api: widget.api,
            browser: widget.browser,
          ),
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
          _feedVisible.value = index == 0;
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
