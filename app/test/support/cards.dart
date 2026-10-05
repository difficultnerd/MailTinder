import 'package:app/api/models/feed.dart';

/// Builders for synthetic Feed cards and pages. Addresses are `example.com`
/// only; messages are invented, never real mail.
FeedCard buildCard({
  String mailboxId = 'mb-1',
  String messageId = 'msg-1',
  String senderName = 'Sender One',
  String senderAddress = 'sender@example.com',
  String subject = 'Subject',
  String preview = 'Body preview text.',
  String bulkReason = 'Mailing list, sent through an email service.',
  DateTime? receivedAt,
  int bulkScore = 0,
  int skipCount = 0,
  MessageClass messageClass = MessageClass.bulkNoHeader,
  bool hasOneClick = false,
  Suggestion? suggestion,
  CategoryRef? keepPrompt,
  BossInfo? boss,
  String providerWebUrl = 'https://mail.google.com/',
  String classificationToken = 'tok-1',
  String? classifierId,
}) {
  return FeedCard(
    mailboxId: mailboxId,
    messageId: messageId,
    senderName: senderName,
    senderAddress: senderAddress,
    subject: subject,
    preview: preview,
    bulkReason: bulkReason,
    receivedAt: receivedAt ?? DateTime.utc(2026, 1, 1, 12),
    bulkScore: bulkScore,
    skipCount: skipCount,
    messageClass: messageClass,
    hasOneClick: hasOneClick,
    suggestion: suggestion,
    keepPrompt: keepPrompt,
    boss: boss,
    providerWebUrl: Uri.parse(providerWebUrl),
    classificationToken: classificationToken,
    classifierId: classifierId,
  );
}

/// An empty page with no cursor: the default terminal feed page.
FeedPage emptyFeedPage({
  String? nextCursor,
  bool phaseChanged = false,
  List<MailboxError> mailboxErrors = const [],
  String phase = 'new',
}) {
  return FeedPage(
    cards: const [],
    nextCursor: nextCursor,
    phase: phase,
    phaseChanged: phaseChanged,
    mailboxErrors: mailboxErrors,
    ruleActionsApplied: 0,
  );
}

/// A page of the given cards.
FeedPage pageOf(
  List<FeedCard> cards, {
  String? nextCursor,
  bool phaseChanged = false,
  List<MailboxError> mailboxErrors = const [],
  String phase = 'new',
  int ruleActionsApplied = 0,
}) {
  return FeedPage(
    cards: cards,
    nextCursor: nextCursor,
    phase: phase,
    phaseChanged: phaseChanged,
    mailboxErrors: mailboxErrors,
    ruleActionsApplied: ruleActionsApplied,
  );
}

MailboxError signInError(String mailboxId) =>
    MailboxError(mailboxId: mailboxId, code: 'mailbox_needs_sign_in');
