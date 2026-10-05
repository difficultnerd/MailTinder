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
}
