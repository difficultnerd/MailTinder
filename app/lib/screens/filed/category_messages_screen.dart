import 'dart:async';

import 'package:flutter/material.dart';

import '../../api/api_client.dart';
import '../../api/models/category.dart';
import '../../api/models/session.dart';
import '../../copy.dart';
import '../../format.dart';
import '../../platform/browser.dart';
import '../../state/filed_model.dart';
import '../../state/session_model.dart';

/// The messages in one category (S9 section 5, API-CAT-5): sender, subject,
/// date and mailbox badge; tapping a message opens it in the provider's own
/// web client.
class CategoryMessagesScreen extends StatefulWidget {
  const CategoryMessagesScreen({
    super.key,
    required this.category,
    required this.api,
    required this.session,
    required this.browser,
  });

  final Category category;
  final ApiClient api;
  final SessionModel session;
  final Browser browser;

  @override
  State<CategoryMessagesScreen> createState() => _CategoryMessagesScreenState();
}

class _CategoryMessagesScreenState extends State<CategoryMessagesScreen> {
  late final CategoryMessagesModel _model;
  final ScrollController _controller = ScrollController();

  @override
  void initState() {
    super.initState();
    _model = CategoryMessagesModel(api: widget.api, category: widget.category);
    _controller.addListener(_maybeLoadMore);
    _model.loadFirst();
  }

  @override
  void dispose() {
    _controller.removeListener(_maybeLoadMore);
    _controller.dispose();
    _model.dispose();
    super.dispose();
  }

  /// Loads the next page when scrolled within 3 rows of the end.
  void _maybeLoadMore() {
    if (!_controller.hasClients) return;
    final position = _controller.position;
    if (position.pixels >= position.maxScrollExtent - 3 * 80) {
      unawaited(_model.loadMore());
    }
  }

  String _mailboxAddress(String mailboxId) {
    for (final mailbox
        in widget.session.session?.mailboxes ?? const <Mailbox>[]) {
      if (mailbox.mailboxId == mailboxId) return mailbox.emailAddress;
    }
    return '';
  }

  void _open(FiledMessage message) {
    final target = safeNavigationTarget(
      message.providerWebUrl.toString(),
      allowLoopbackHttp: kE2eBuild,
    );
    if (target != null) {
      widget.browser.openExternal(target);
    }
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      appBar: AppBar(title: Text(widget.category.name)),
      body: ListenableBuilder(
        listenable: _model,
        builder: (context, _) {
          if (_model.state == LoadState.loading) {
            return Center(
              child: Semantics(
                label: Copy.loading,
                child: const CircularProgressIndicator(),
              ),
            );
          }
          if (_model.state == LoadState.failed) {
            return Center(
              child: Padding(
                padding: const EdgeInsets.all(24),
                child: Column(
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    const Text(Copy.actionFailed, textAlign: TextAlign.center),
                    const SizedBox(height: 16),
                    Semantics(
                      label: Copy.tryAgain,
                      button: true,
                      child: ElevatedButton(
                        onPressed: _model.loadFirst,
                        child: const Text(Copy.tryAgain),
                      ),
                    ),
                  ],
                ),
              ),
            );
          }
          return Column(
            children: [
              for (final error in _model.mailboxErrors)
                _MailboxBanner(
                  text: Copy.filedMailboxUnavailable(
                    _mailboxAddress(error.mailboxId),
                  ),
                ),
              Expanded(
                child: ListView.builder(
                  controller: _controller,
                  itemCount: _model.messages.length,
                  itemBuilder: (context, index) {
                    final message = _model.messages[index];
                    return ListTile(
                      title: Text(message.senderName),
                      subtitle: Text(
                        message.subject,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                      ),
                      trailing: Column(
                        mainAxisSize: MainAxisSize.min,
                        crossAxisAlignment: CrossAxisAlignment.end,
                        children: [
                          Text(formatDate(message.receivedAt)),
                          Text(_mailboxAddress(message.mailboxId)),
                        ],
                      ),
                      onTap: () => _open(message),
                    );
                  },
                ),
              ),
            ],
          );
        },
      ),
    );
  }
}

class _MailboxBanner extends StatelessWidget {
  const _MailboxBanner({required this.text});

  final String text;

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    return Material(
      color: scheme.errorContainer,
      child: Padding(
        padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 8),
        child: Text(text, style: TextStyle(color: scheme.onErrorContainer)),
      ),
    );
  }
}
