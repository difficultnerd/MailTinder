import 'dart:async';

import 'package:flutter/material.dart';

import '../../api/api_client.dart';
import '../../api/models/category.dart';
import '../../copy.dart';
import '../../format.dart';
import '../../platform/browser.dart';
import '../../routes.dart';
import '../../state/filed_model.dart';
import '../../state/session_model.dart';

/// The Filed tab (S9 section 5, FL-05 AC1): the user's categories with message
/// counts across all mailboxes, rename and delete (label only, never
/// messages).
///
/// When [model] is null the screen renders the placeholder: the HomeShell
/// builds it without dependencies until a later task wires the live model.
class FiledScreen extends StatefulWidget {
  const FiledScreen({
    super.key,
    this.model,
    this.session,
    this.api,
    this.browser,
  });

  final FiledModel? model;
  final SessionModel? session;
  final ApiClient? api;
  final Browser? browser;

  @override
  State<FiledScreen> createState() => _FiledScreenState();
}

class _FiledScreenState extends State<FiledScreen> {
  @override
  void initState() {
    super.initState();
    widget.model?.load();
  }

  @override
  Widget build(BuildContext context) {
    final model = widget.model;
    if (model == null) {
      return const Center(child: Text(Copy.tabFiled));
    }

    return ListenableBuilder(
      listenable: model,
      builder: (context, _) {
        return switch (model.state) {
          LoadState.loading => Center(
            child: Semantics(
              label: Copy.loading,
              child: const CircularProgressIndicator(),
            ),
          ),
          LoadState.failed => Center(
            child: Column(
              mainAxisSize: MainAxisSize.min,
              children: [
                const Text(Copy.actionFailed, textAlign: TextAlign.center),
                const SizedBox(height: 16),
                Semantics(
                  label: Copy.tryAgain,
                  button: true,
                  child: ElevatedButton(
                    onPressed: model.load,
                    child: const Text(Copy.tryAgain),
                  ),
                ),
              ],
            ),
          ),
          LoadState.ready =>
            model.categories.isEmpty
                ? const Center(child: Text(Copy.filedEmpty))
                : ListView(
                    children: [
                      for (final category in model.categories)
                        _categoryRow(context, model, category),
                    ],
                  ),
        };
      },
    );
  }

  Widget _categoryRow(
    BuildContext context,
    FiledModel model,
    Category category,
  ) {
    return ListTile(
      title: Text(category.name),
      subtitle: Text(formatCount(category.messageCount)),
      onTap: () => Navigator.of(
        context,
      ).pushNamed(Routes.filedCategory, arguments: category),
      trailing: Semantics(
        label: Copy.moreFor(category.name),
        button: true,
        child: PopupMenuButton<String>(
          onSelected: (value) {
            switch (value) {
              case 'rename':
                unawaited(_rename(context, model, category));
              case 'delete':
                unawaited(_confirmDelete(context, model, category));
            }
          },
          itemBuilder: (context) => [
            const PopupMenuItem<String>(
              value: 'rename',
              child: Text(Copy.rename),
            ),
            const PopupMenuItem<String>(
              value: 'delete',
              child: Text(Copy.delete),
            ),
          ],
        ),
      ),
    );
  }

  Future<void> _rename(
    BuildContext context,
    FiledModel model,
    Category category,
  ) async {
    final controller = TextEditingController(text: category.name);
    try {
      await showDialog<void>(
        context: context,
        builder: (dialogContext) {
          String? error;
          return StatefulBuilder(
            builder: (builderContext, setDialogState) {
              return AlertDialog(
                title: const Text(Copy.rename),
                content: Column(
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    TextField(
                      controller: controller,
                      autofocus: true,
                      decoration: const InputDecoration(
                        labelText: Copy.categoryNameHint,
                      ),
                    ),
                    if (error != null)
                      Padding(
                        padding: const EdgeInsets.only(top: 8),
                        child: Text(error!),
                      ),
                  ],
                ),
                actions: [
                  TextButton(
                    onPressed: () => Navigator.of(dialogContext).pop(),
                    child: const Text(Copy.cancel),
                  ),
                  TextButton(
                    onPressed: () async {
                      final message = await model.rename(
                        category,
                        controller.text,
                      );
                      if (message == null) {
                        if (dialogContext.mounted) {
                          Navigator.of(dialogContext).pop();
                        }
                        return;
                      }
                      setDialogState(() {
                        error = message;
                      });
                    },
                    child: const Text(Copy.rename),
                  ),
                ],
              );
            },
          );
        },
      );
    } finally {
      controller.dispose();
    }
  }

  Future<void> _confirmDelete(
    BuildContext context,
    FiledModel model,
    Category category,
  ) async {
    final confirmed = await showDialog<bool>(
      context: context,
      builder: (dialogContext) => AlertDialog(
        title: Text(Copy.deleteCategoryQuestion(category.name)),
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
    if (confirmed != true) return;
    await model.delete(category);
  }
}
