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
}
