import 'package:flutter/material.dart';

import '../../api/models/admin.dart';
import '../../copy.dart';
import '../../format.dart';
import '../../state/admin_model.dart';
import '../../state/filed_model.dart' show LoadState;
import '../settings/app_scope.dart';

/// S9 7.6: invites, invite requests and users. Every write goes through
/// Confirm it's you; the server enforces `admin`, the hide is cosmetic.
class AdminScreen extends StatelessWidget {
  const AdminScreen({super.key});

  @override
  Widget build(BuildContext context) {
    final scope = AppScope.maybeOf(context)!;
    return ListenableBuilder(
      listenable: scope.session,
      builder: (context, _) {
        final isAdmin = scope.session.session?.user?.isAdmin ?? false;
        if (!isAdmin) {
          return Scaffold(
            appBar: AppBar(title: const Text(Copy.admin)),
            body: const Center(child: Text(Copy.adminsOnly)),
          );
        }
        return const _AdminTabs();
      },
    );
  }
}

class _AdminTabs extends StatefulWidget {
  const _AdminTabs();

  @override
  State<_AdminTabs> createState() => _AdminTabsState();
}

class _AdminTabsState extends State<_AdminTabs> {
  PagedList<Invite>? _invites;
  PagedList<InviteRequest>? _requests;
  PagedList<AdminUser>? _users;

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    if (_invites != null) {
      return;
    }
    final api = AppScope.maybeOf(context)!.api;
    _invites = PagedList<Invite>((c) => api.listInvites(cursor: c))..load();
    _requests = PagedList<InviteRequest>(
      (c) => api.listInviteRequests(cursor: c),
    )..load();
    _users = PagedList<AdminUser>((c) => api.listUsers(cursor: c))..load();
  }

  @override
  void dispose() {
    _invites?.dispose();
    _requests?.dispose();
    _users?.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return DefaultTabController(
      length: 3,
      child: Scaffold(
        appBar: AppBar(
          title: const Text(Copy.admin),
          bottom: const TabBar(
            tabs: [
              Tab(text: Copy.adminInvites),
              Tab(text: Copy.adminRequests),
              Tab(text: Copy.adminUsers),
            ],
          ),
        ),
        body: TabBarView(
          children: [
            _InvitesTab(list: _invites!),
            _RequestsTab(list: _requests!),
            _UsersTab(list: _users!),
          ],
        ),
      ),
    );
  }
}

/// Shared behaviour for the three tabs: step-up writes, toasts, confirm.
mixin _AdminActions<T extends StatefulWidget> on State<T> {
  AppScope get scope => AppScope.maybeOf(context)!;

  void toast(String text) {
    ScaffoldMessenger.of(context).showSnackBar(SnackBar(content: Text(text)));
  }

  /// Runs an admin write behind Confirm it's you. Returns true when it ran.
  Future<bool> write(String label, Future<void> Function() action) async {
    try {
      final done = await scope.stepUp.run<bool>(
        waitingActionLabel: label,
        action: () async {
          await action();
          return true;
        },
      );
      return done == true;
    } on Object {
      if (mounted) {
        toast(Copy.actionFailed);
      }
      return false;
    }
  }

  Future<bool> confirm(String question, String yes) async {
    final ok = await showDialog<bool>(
      context: context,
      builder: (dialogContext) => AlertDialog(
        content: Text(question),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(dialogContext).pop(false),
            child: const Text(Copy.cancel),
          ),
          TextButton(
            onPressed: () => Navigator.of(dialogContext).pop(true),
            child: Text(yes),
          ),
        ],
      ),
    );
    return ok == true && mounted;
  }
}

Widget _listBody<T>(
  PagedList<T> list,
  Widget Function(T) row, {
  List<Widget> header = const [],
}) {
  return ListenableBuilder(
    listenable: list,
    builder: (context, _) {
      if (list.state == LoadState.failed) {
        return Center(
          child: Column(
            mainAxisSize: MainAxisSize.min,
            children: [
              const Text(Copy.actionFailed),
              TextButton(
                onPressed: list.load,
                child: const Text(Copy.tryAgain),
              ),
            ],
          ),
        );
      }
      if (list.state == LoadState.loading) {
        return const Center(child: CircularProgressIndicator());
      }
      return ListView(
        children: [
          ...header,
          if (list.items.isEmpty)
            const ListTile(title: Text(Copy.adminNothingHere)),
          for (final item in list.items) row(item),
          if (list.hasMore)
            TextButton(
              onPressed: list.loadingMore ? null : list.loadMore,
              child: const Text(Copy.loadMore),
            ),
        ],
      );
    },
  );
}

bool _validEmail(String raw) {
  final s = raw.trim();
  if (s.isEmpty || s.length > 320) {
    return false;
  }
  final parts = s.split('@');
  return parts.length == 2 && parts[0].isNotEmpty && parts[1].isNotEmpty;
}

class _InvitesTab extends StatefulWidget {
  const _InvitesTab({required this.list});

  final PagedList<Invite> list;

  @override
  State<_InvitesTab> createState() => _InvitesTabState();
}

class _InvitesTabState extends State<_InvitesTab> with _AdminActions {
  final _field = TextEditingController();
  String? _error;

  @override
  void dispose() {
    _field.dispose();
    super.dispose();
  }

  Future<void> _invite() async {
    final address = _field.text.trim();
    if (!_validEmail(address)) {
      setState(() => _error = Copy.emailInvalid);
      return;
    }
    setState(() => _error = null);
    final ok = await write(
      Copy.stepUpInvite(address),
      () => scope.api.createInvite(address),
    );
    if (ok && mounted) {
      _field.clear();
      await widget.list.load();
    }
  }

  Future<void> _resend(Invite i) async {
    final ok = await write(
      Copy.stepUpResend(i.emailAddress),
      () => scope.api.resendInvite(i.inviteId),
    );
    if (ok && mounted) {
      await widget.list.load();
    }
  }

  Future<void> _revoke(Invite i) async {
    if (!await confirm(Copy.revokeQuestion(i.emailAddress), Copy.revoke)) {
      return;
    }
    final ok = await write(
      Copy.stepUpRevoke(i.emailAddress),
      () => scope.api.revokeInvite(i.inviteId),
    );
    if (ok && mounted) {
      await widget.list.load();
    }
  }

  Widget _row(Invite i) {
    final canResend =
        i.status == InviteStatus.pending || i.status == InviteStatus.expired;
    return ListTile(
      title: Text(i.emailAddress),
      subtitle: Text(
        '${Copy.inviteStatus(i.status)}, ${formatDate(i.lastSentAt)}',
      ),
      trailing: Wrap(
        children: [
          if (canResend)
            TextButton(
              onPressed: () => _resend(i),
              child: const Text(Copy.resend),
            ),
          if (i.status == InviteStatus.pending)
            TextButton(
              onPressed: () => _revoke(i),
              child: const Text(Copy.revoke),
            ),
        ],
      ),
    );
  }

  @override
  Widget build(BuildContext context) {
    return _listBody<Invite>(
      widget.list,
      _row,
      header: [
        Padding(
          padding: const EdgeInsets.all(16),
          child: Row(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Expanded(
                child: TextField(
                  controller: _field,
                  keyboardType: TextInputType.emailAddress,
                  decoration: InputDecoration(
                    labelText: Copy.emailAddressLabel,
                    errorText: _error,
                  ),
                ),
              ),
              const SizedBox(width: 8),
              FilledButton(
                onPressed: _invite,
                child: const Text(Copy.inviteButton),
              ),
            ],
          ),
        ),
      ],
    );
  }
}

class _RequestsTab extends StatefulWidget {
  const _RequestsTab({required this.list});

  final PagedList<InviteRequest> list;

  @override
  State<_RequestsTab> createState() => _RequestsTabState();
}

class _RequestsTabState extends State<_RequestsTab> with _AdminActions {
  Future<void> _approve(InviteRequest r) async {
    final ok = await write(
      Copy.stepUpApprove(r.emailAddress),
      () => scope.api.approveInviteRequest(r.requestId),
    );
    if (ok && mounted) {
      widget.list.removeWhere((x) => x.requestId == r.requestId);
    }
  }

  Future<void> _decline(InviteRequest r) async {
    if (!await confirm(Copy.declineQuestion(r.emailAddress), Copy.decline)) {
      return;
    }
    final ok = await write(
      Copy.stepUpDecline(r.emailAddress),
      () => scope.api.declineInviteRequest(r.requestId),
    );
    if (ok && mounted) {
      widget.list.removeWhere((x) => x.requestId == r.requestId);
    }
  }

  @override
  Widget build(BuildContext context) {
    return _listBody<InviteRequest>(
      widget.list,
      (r) => ListTile(
        title: Text(r.emailAddress),
        subtitle: Text(formatDateTime(r.createdAt)),
        trailing: Wrap(
          children: [
            TextButton(
              onPressed: () => _approve(r),
              child: const Text(Copy.approve),
            ),
            TextButton(
              onPressed: () => _decline(r),
              child: const Text(Copy.decline),
            ),
          ],
        ),
      ),
    );
  }
}

class _UsersTab extends StatefulWidget {
  const _UsersTab({required this.list});

  final PagedList<AdminUser> list;

  @override
  State<_UsersTab> createState() => _UsersTabState();
}

class _UsersTabState extends State<_UsersTab> with _AdminActions {
  Future<void> _endSession(AdminUser u) async {
    if (!await confirm(
      Copy.endSessionQuestion(u.emailAddress),
      Copy.endSession,
    )) {
      return;
    }
    final ok = await write(
      Copy.stepUpEndSession(u.emailAddress),
      () => scope.api.endUserSession(u.userId),
    );
    if (ok && mounted) {
      await widget.list.load();
    }
  }

  @override
  Widget build(BuildContext context) {
    return _listBody<AdminUser>(
      widget.list,
      (u) => ListTile(
        title: Text(u.emailAddress),
        subtitle: Text(Copy.mailboxCount(u.mailboxCount)),
        trailing: TextButton(
          onPressed: u.signedIn ? () => _endSession(u) : null,
          child: const Text(Copy.endSession),
        ),
      ),
    );
  }
}
