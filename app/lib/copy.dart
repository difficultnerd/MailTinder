import 'api/models/admin.dart';
import 'format.dart';
import 'state/swipe_controller.dart';

abstract final class Copy {
  static const productName = 'Mail Tinder';
  static const productDescription =
      'Triage your email like a dating app. Swipe through lists, newsletters and receipts in seconds.';
  static const tagline =
      'Swipe through your inbox and clear the mail you don\'t want.';
  static const tabFeed = 'Feed';
  static const tabFiled = 'Filed';
  static const tabNeedsAttention = 'Needs Attention';
  static const tabSettings = 'Settings';
  static const genericError = 'Something went wrong. Try again.';
  static const offline =
      "You're offline. Swipes are paused until you're back online.";
  static const loading = 'Loading';
  static const tryAgain = 'Try again';
  static const continueWithGoogle = 'Continue with Google';
  static const requestAnInvite = 'Request an invite';
  static const requestInvite = 'Request an invite';
  static const privacyNotice = 'Privacy notice';
  static const scopesExplainer =
      'Google will ask you to let Mail Tinder read and organise your Gmail, '
      'send unsubscribe emails for you, and keep its own settings file in your '
      'Google Drive.';
  static const invited =
      'You\'ve been invited. Continue with the Google account the invite was '
      'sent to.';
  static const inviteInvalid =
      'This invite link no longer works. Ask for a new invite.';
  static const emailMismatch =
      'This account isn\'t the one you were invited with. Try the invited '
      'account, or ask for a new invite.';
  static const emailUnverified =
      'We couldn\'t confirm this account\'s email address. Try another account.';
  static const notRegistered =
      'There\'s no Mail Tinder account for this Google account. Use your '
      'invite link, or request an invite.';
  static const signInFailed = 'Sign-in didn\'t finish. Try again.';
  static const inviteOnlyExplainer =
      'Mail Tinder is invite only. Send a request and you\'ll hear back by '
      'email if it\'s approved.';
  static const useDifferentAccount = 'Use a different account';
  static const requestSent =
      'Request sent. You\'ll get an email if it\'s approved.';
  static const tooManyRequests = 'Too many requests. Try again later.';
  static const actionFailed = 'Couldn\'t do that. Try again.';
  static const confirmItsYou = 'Confirm it\'s you';
  static const stepUpExplainer = 'Google will ask you to sign in again.';
  static const cancel = 'Cancel';
  static const stepUpWrongAccount =
      'That Google account isn\'t linked to your Mail Tinder account. '
      'Nothing was changed.';
  static const stepUpNotConfirmed = 'Not confirmed. Nothing was changed.';
  static const stepUpConfirmedPopup = 'Confirmed. You can close this window.';
  static const popupBlocked =
      'Your browser blocked the sign-in window. Allow pop-ups for Mail '
      'Tinder, then try again.';
  static const close = 'Close';
  static const upToDate =
      "You're up to date. Now working back through older mail.";
  static const continueLabel = 'Continue';
  static const nothingToTriage = 'Nothing to triage.';
  static const signInAgain = 'Sign in again';
  static const allNeedSignIn = 'Sign in again to see your mail.';
  static const loadingCards = 'Loading cards';

  static const kept = 'Kept.';
  static const skipped = "Skipped. It'll come back later.";
  static const trashed = 'Trashed.';
  static const trashedUnsubscribeManual =
      'Trashed. The unsubscribe link is in Needs Attention.';
  static const trashedListNoUnsubscribe =
      'Trashed. Future mail from this sender will be trashed too.';
  static const reportedSpam = 'Reported as spam and trashed.';
  static const restored = 'Restored.';
  static const restoredAlreadySent =
      "Restored. The unsubscribe request had already been sent.";
  static const restoredSpam =
      "Restored. The spam report itself can't be recalled.";
  static const cannotUndo = "Can't undo that any more.";
  static const block = 'Block';
  static const notNow = 'Not now';
  static const blocked = 'Blocked.';
  static const keepButton = 'Keep';
  static const rejectButton = 'Reject';
  static const fileButton = 'File';
  static const skipButton = 'Skip';
  static const undoButton = 'Undo';

  static const filedEmpty = 'Swipe up on an email to start filing.';
  static const rename = 'Rename';
  static const delete = 'Delete';
  static const categoryNameHint = 'Category name';
  static const categoryNameRule =
      'Use 1 to 100 characters, not starting or ending with /.';

  static const newCategory = 'New category';
  static const other = 'Other';
  static const dismiss = 'Dismiss';

  static const nothingNeedsYou = 'Nothing needs you.';
  static const openUnsubscribePage = 'Open unsubscribe page';
  static const done = 'Done';

  static const naHttpsOnlyUnsubscribe =
      'This sender needs you to unsubscribe on their website.';
  static const naOneClickRedirect =
      'The unsubscribe request was redirected, so we stopped. '
      'Open the page to finish.';
  static const naOneClickAddressRefused =
      "We couldn't safely send this unsubscribe request. "
      "Check the sender's own unsubscribe options.";
  static const naUnsubscribeIgnored = 'Mail still arriving after unsubscribe';
  static const naUnsubscribeFailed =
      "We couldn't unsubscribe you from this sender. "
      'Open the page to finish.';
  static const naJobExpired =
      "The unsubscribe request didn't finish in time. "
      'Open the page to finish.';
  static const naOther = 'This needs your attention.';

  /// {"Couldn't unsubscribe from $sender. Sign in again to $address."}
  static String naMailboxNeedsSignIn(String sender, String address) =>
      "Couldn't unsubscribe from $sender. Sign in again to $address.";

  /// {"File under $n"} (FL-03 AC1).
  static String fileUnder(String name) => 'File under $name';

  /// {"You always keep these. File under $n?"} (FL-04 AC1).
  static String keepPrompt(String name) =>
      'You always keep these. File under $name?';

  /// {"You already have a category called $n."}
  static String categoryExists(String name) =>
      'You already have a category called $name.';

  /// {"Delete $n? The label is removed from your mailboxes. Your messages
  /// stay where they are."}
  static String deleteCategoryQuestion(String name) =>
      'Delete $name? The label is removed from your mailboxes. '
      'Your messages stay where they are.';

  /// {"Can't reach $a right now. Some messages may be missing."}
  static String filedMailboxUnavailable(String address) =>
      "Can't reach $address right now. Some messages may be missing.";

  /// Screen reader label for a category row's trailing menu:
  /// {"More for $n"}.
  static String moreFor(String name) => 'More for $name';

  /// {"Can't reach $a. Sign in again"}
  static String mailboxNeedsSignIn(String address) =>
      "Can't reach $address. Sign in again";

  /// {"Can't reach $a right now. Pull down to try again."}
  static String mailboxUnavailable(String address) =>
      "Can't reach $address right now. Pull down to try again.";

  /// {"Sign in again to $a"}
  static String signInAgainTo(String address) => 'Sign in again to $address';

  /// {"Bulk $s"}
  static String bulkBadge(int score) => 'Bulk $score';

  /// Semantics for the bulk badge: "Bulk score $s out of 100. Tap for the
  /// reason."
  static String bulkBadgeSemantics(int score) =>
      'Bulk score $score out of 100. Tap for the reason.';

  /// {"Trashed. Unsubscribing in $d."} where $d is the delay in minutes,
  /// at least 1, "1 minute" singular. Minutes are rounded up (ceil) so a
  /// 4:59 remaining delay reads "5 minutes" (S9).
  static String trashedUnsubscribing(Duration delay) {
    final minutes = (delay.inSeconds / 60).ceil();
    final m = minutes < 1 ? 1 : minutes;
    final unit = m == 1 ? 'minute' : 'minutes';
    return 'Trashed. Unsubscribing in $m $unit.';
  }

  /// {"Filed under $name."}
  static String filed(String name) => 'Filed under $name.';

  /// {"You've rejected $n 3 times. Block them?"} (3 from
  /// `kPersonalBlockThreshold`).
  static String blockQuestion(String name) =>
      "You've rejected $name $kPersonalBlockThreshold times. Block them?";

  // Settings, Connected accounts and Account (S9 7, 7.1, 7.5).
  static const settingsConnectedAccounts = 'Connected accounts';
  static const settingsAccount = 'Account';
  static const addGmail = 'Add Gmail';
  static const disconnect = 'Disconnect';
  static const statusConnected = 'Connected';
  static const statusNeedsSignIn = 'Needs sign-in';
  static const mailboxAdded = 'Mailbox added.';
  static const mailboxLinkedElsewhere =
      'That Google account is already linked to another Mail Tinder account. Nothing was linked.';
  static const onlyMailbox =
      'This is your only mailbox. To remove it, delete your account instead.';
  static const goToAccount = 'Go to Account';
  static const appFolderMoveFailed =
      "Couldn't move your Mail Tinder data to another mailbox, so nothing was disconnected. Try again.";
  static const stepUpAddGmail = 'to add a Gmail account';
  static const stepUpDeleteAccount = 'to delete your account';
  static const sounds = 'Sounds';
  static const signOut = 'Sign out';
  static const deleteAccount = 'Delete account';
  static const deleteAccountExplain =
      'Delete your Mail Tinder account? We delete your Mail Tinder settings file from your Google Drive, cancel queued unsubscribes, disconnect your mailboxes and destroy your encryption key. Labels already on your messages stay in your mailbox.';
  static const deleteAccountConfirm =
      "This can't be undone. Delete your account now?";
  static const appFoldersNotDeleted =
      "We couldn't delete your Mail Tinder settings file from these mailboxes. Remove it in Google Drive, under Settings, Manage apps:";
  static const accountDeleted = 'Your account is deleted.';
  static const continueButton = 'Continue';

  static String disconnectQuestion(String a) =>
      'Disconnect $a? Its cards leave your Feed and its queued unsubscribes are cancelled.';
  static String stepUpDisconnect(String a) => 'to disconnect $a';

  // History, Rules and Stats (S9 7.2 to 7.4).
  static const history = 'History';
  static const rules = 'Rules';
  static const stats = 'Stats';
  static const historyEmpty = 'Nothing here yet.';
  static const historyAll = 'All';
  static const historyUnsubscribes = 'Unsubscribes';
  static const historyRuleActions = 'Rule actions';
  static const historyFiling = 'Filing';
  static const rulesRejectList = 'Reject list';
  static const rulesBlockedPeople = 'Blocked people';
  static const rulesFiling = 'Filing rules';
  static const ruleOnSwitch = 'Rule on';
  static const deleteRuleQuestion =
      'Delete this rule? It stops acting on new mail.';
  static const emailsTriaged = 'Emails triaged';
  static const sendersUnsubscribed = 'Senders unsubscribed';
  static const unsubscribesConfirmed = 'Unsubscribes confirmed working';
  static const achievementUnknown = 'Achievement';
  static const yearlyUnknown = 'Emails a year stopped: unknown';

  static String yearlyStopped(int n) => 'About $n emails a year stopped';
  static String actedOn(int n) => 'Acted on $n messages';
  static String mailStoppedTotal(int n) =>
      'About ${formatCount(n)} emails a year stopped';
  static String achievementLocked(String title) => '$title, locked';

  /// History action words.
  static String historyAction(String action) => switch (action) {
    'trashed_by_rule' => 'Trashed by rule',
    'unsubscribe' => 'Unsubscribe',
    'filed' => 'Filed',
    'filed_by_rule' => 'Filed by rule',
    'blocked' => 'Blocked',
    'reported_spam' => 'Reported as spam',
    _ => action,
  };

  /// History outcome words.
  static String historyOutcome(String outcome) => switch (outcome) {
    'sent' => 'Sent',
    'needs_attention' => 'Needs attention',
    'failed' => 'Failed',
    'cancelled' => 'Cancelled',
    'expired' => 'Expired',
    'done' => 'Done',
    _ => outcome,
  };

  // Experiments and Admin (S9 7.6, 7.8).
  static const experiments = 'Experiments';
  static const experimentsSwitch = 'Experiments';
  static const admin = 'Admin';
  static const consentChanged =
      'The consent text has changed. Read it again before you switch this on.';
  static const experimentPaused = 'The experiment is paused';
  static const experimentsOffQuestion =
      'Turn this off? Your experiment records will be deleted. Anonymous totals already published or saved stay as they are.';
  static const turnOff = 'Turn off';
  static const adminsOnly = 'Admins only.';
  static const emailInvalid = 'Enter a valid email address.';
  static const emailAddressLabel = 'Email address';
  static const inviteButton = 'Invite';
  static const resend = 'Re-send';
  static const revoke = 'Revoke';
  static const approve = 'Approve';
  static const decline = 'Decline';
  static const endSession = 'End session';
  static const loadMore = 'Load more';
  static const adminInvites = 'Invites';
  static const adminRequests = 'Requests';
  static const adminUsers = 'Users';
  static const adminNothingHere = 'Nothing here yet.';

  static String stepUpInvite(String a) => 'to invite $a';
  static String stepUpResend(String a) => 'to re-send the invite to $a';
  static String stepUpRevoke(String a) => 'to revoke the invite to $a';
  static String stepUpApprove(String a) => 'to approve $a';
  static String stepUpDecline(String a) => 'to decline $a';
  static String stepUpEndSession(String a) => "to end $a's session";
  static String endSessionQuestion(String a) =>
      "End $a's session? They'll need to sign in again.";
  static String revokeQuestion(String a) =>
      'Revoke the invite to $a? The link stops working.';
  static String declineQuestion(String a) => 'Decline $a? They are not told.';
  static String mailboxCount(int n) => n == 1 ? '1 mailbox' : '$n mailboxes';
  static String inviteStatus(InviteStatus s) => switch (s) {
    InviteStatus.pending => 'Pending',
    InviteStatus.used => 'Used',
    InviteStatus.revoked => 'Revoked',
    InviteStatus.expired => 'Expired',
  };

  // Bake-off report (S9 7.7, S7 section 4).
  static const bakeoffReport = 'Bake-off report';
  static const tooFewToShow = 'Too few to show';
  static const notEnoughData =
      'Not enough data yet. Come back when more swipes are in.';
  static const pickOneVersion = 'Pick one version';
  static const snapshotSaved = 'Snapshot saved.';
  static const deleteSnapshotFirst = 'Delete a snapshot first';
  static const saveSnapshot = 'Save snapshot';
  static const downloadCsv = 'Download CSV';
  static const stepUpSaveSnapshot = 'to save a snapshot';
  static const stepUpDeleteSnapshot = 'to delete a snapshot';
  static const bakeoffFrom = 'From';
  static const bakeoffTo = 'To';
  static const bakeoffInputVersion = 'Input version';
  static const bakeoffQuestionVersion = 'Question version';
  static const bakeoffAnyVersion = 'Any';
  static const bakeoffSnapshotName = 'Snapshot name';
  static const bakeoffSnapshots = 'Snapshots';
  static const bakeoffNoSnapshots = 'No snapshots yet.';
  static const bakeoffKillSwitches = 'Kill switches';
  static const bakeoffSnapshotNameInvalid =
      'Enter a name of 1 to 100 characters.';
  static String deleteSnapshotQuestion(String n) => 'Delete the snapshot $n?';
  static String modelLabel(String m) => switch (m) {
    'header_rules' => 'Header rules',
    'gemini' => 'Gemini',
    'jev' => 'Jev',
    _ => m,
  };
  static String stepUpKillSwitch(String model, bool on) =>
      'to switch ${modelLabel(model)} ${on ? 'on' : 'off'}';
  static String snapshotCards(int? n) => n == null ? tooFewToShow : '$n cards';

  static String meter(int count, int change) {
    final base = formatCount(count);
    if (change < 0) return '$base, down ${formatCount(-change)} today';
    if (change > 0) return '$base, up ${formatCount(change)} today';
    return '$base, no change today';
  }

  static const meterPartial = "Some mailboxes didn't answer";
  static String level(int year, int remaining) =>
      'Level $year: ${formatCount(remaining)} left';
  static String levelComplete(int oldYear, int newYear) =>
      'Level $oldYear complete. Level $newYear unlocked.';
  static String achievementUnlocked(String title) =>
      'Achievement unlocked: $title';
  static String bossDefeated(String name) => 'Boss defeated: $name';
  static String bossLabel(String name, int remaining) =>
      'Boss $name: $remaining left';
  static const roundOver = 'Round over';
  static const roundCleared = 'Cleared';
  static const roundKept = 'Kept';
  static const roundFiled = 'Filed';
  static const roundUnsubscribed = 'Senders unsubscribed';
  static const roundBlocked = 'Senders blocked';
  static const keepGoing = 'Keep going';
}
