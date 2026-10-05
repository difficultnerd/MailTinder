/// S7 models for the Needs Attention tab: `NeedsAttentionItem`,
/// `NeedsAttentionPage` and `NaReason` (API-NA-1 to API-NA-3, S7 5.8).
library;

/// Why an item needs attention (S7 `reason_code`).
///
/// v1 reasons are the first seven; `captcha`, `login_required` and the
/// `page_*` codes are reserved for the v2 page handler and map to [other].
enum NaReason {
  httpsOnlyUnsubscribe,
  oneClickRedirect,
  oneClickAddressRefused,
  unsubscribeFailed,
  unsubscribeIgnored,
  jobExpired,
  mailboxNeedsSignIn,
  other,
}

/// Maps the wire `reason_code` to [NaReason]; v2 codes and anything unknown
/// become [NaReason.other].
NaReason parseNaReason(String raw) {
  return switch (raw) {
    'https_only_unsubscribe' => NaReason.httpsOnlyUnsubscribe,
    'one_click_redirect' => NaReason.oneClickRedirect,
    'one_click_address_refused' => NaReason.oneClickAddressRefused,
    'unsubscribe_failed' => NaReason.unsubscribeFailed,
    'unsubscribe_ignored' => NaReason.unsubscribeIgnored,
    'job_expired' => NaReason.jobExpired,
    // S7 has no reason code for UN-01 AC6 yet; the api sends
    // `sign_in_required` (S7 6 `NeedsAttentionItem.reason_code`).
    'sign_in_required' => NaReason.mailboxNeedsSignIn,
    _ => NaReason.other,
  };
}

/// Whether the reason shows the "Open unsubscribe page" action (S9 section 6).
bool naReasonAllowsOpen(NaReason reason) {
  return switch (reason) {
    NaReason.httpsOnlyUnsubscribe => true,
    NaReason.oneClickRedirect => true,
    // The target was a private address: there is nothing safe to open.
    NaReason.oneClickAddressRefused => false,
    NaReason.unsubscribeIgnored => true,
    NaReason.unsubscribeFailed => true,
    NaReason.jobExpired => true,
    // "Sign in again" replaces the open action (UN-01 AC6).
    NaReason.mailboxNeedsSignIn => false,
    NaReason.other => true,
  };
}

/// Applies the mailbox-state rule from the task: `unsubscribe_failed` is shown
/// as [NaReason.mailboxNeedsSignIn] when the item's mailbox is signed out.
NaReason effectiveNaReason(
  NaReason reason, {
  required bool mailboxNeedsSignIn,
}) {
  if (reason == NaReason.unsubscribeFailed && mailboxNeedsSignIn) {
    return NaReason.mailboxNeedsSignIn;
  }
  return reason;
}

/// One open Needs Attention item (S7 `NeedsAttentionItem`).
class NeedsAttentionItem {
  const NeedsAttentionItem({
    required this.itemId,
    required this.mailboxId,
    required this.senderDisplay,
    required this.reason,
    required this.createdAt,
    this.link,
  });

  factory NeedsAttentionItem.fromJson(Map<String, Object?> json) {
    final rawLink = json['link'] as String?;
    final link = rawLink == null ? null : Uri.tryParse(rawLink);
    return NeedsAttentionItem(
      itemId: (json['item_id'] as String?) ?? '',
      mailboxId: (json['mailbox_id'] as String?) ?? '',
      senderDisplay: (json['sender_display'] as String?) ?? '',
      reason: parseNaReason((json['reason_code'] as String?) ?? ''),
      link: link,
      createdAt: DateTime.parse((json['created_at'] as String?) ?? '').toUtc(),
    );
  }

  final String itemId;
  final String mailboxId;
  final String senderDisplay;
  final NaReason reason;

  /// The server-provided unsubscribe link, or null. Only ever opened after a
  /// https check (UN-04 AC6).
  final Uri? link;
  final DateTime createdAt;
}

/// One page of open items with the badge count (S7 API-NA-1).
class NeedsAttentionPage {
  const NeedsAttentionPage({
    required this.items,
    required this.openCount,
    this.nextCursor,
  });

  factory NeedsAttentionPage.fromJson(Map<String, Object?> json) {
    final rawItems = json['items'];
    final items = rawItems is List<Object?>
        ? rawItems
              .whereType<Map<String, Object?>>()
              .map(NeedsAttentionItem.fromJson)
              .toList(growable: false)
        : const <NeedsAttentionItem>[];
    return NeedsAttentionPage(
      items: items,
      openCount: (json['open_count'] as num?)?.toInt() ?? items.length,
      nextCursor: json['next_cursor'] as String?,
    );
  }

  final List<NeedsAttentionItem> items;
  final int openCount;
  final String? nextCursor;
}
