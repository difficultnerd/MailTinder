/// Auth intent and outcome models (S7 3.4).
library;

enum AuthIntent { signIn, join, link, reconnect, stepUp }

extension AuthIntentWire on AuthIntent {
  String get wire => switch (this) {
    AuthIntent.signIn => 'sign_in',
    AuthIntent.join => 'join',
    AuthIntent.link => 'link',
    AuthIntent.reconnect => 'reconnect',
    AuthIntent.stepUp => 'step_up',
  };
}

enum AuthOutcome {
  signedIn,
  joined,
  linked,
  reconnected,
  steppedUp,
  notInvited,
  notRegistered,
  inviteInvalid,
  emailMismatch,
  emailUnverified,
  mailboxLinkedElsewhere,
  stepUpWrongAccount,
  consentBlocked,
  cancelled,
  failed,
  unknown,
}

/// Maps the exact S7 3.4 outcome strings; anything else -> [AuthOutcome.unknown].
AuthOutcome parseAuthOutcome(String? raw) {
  return switch (raw) {
    'signed_in' => AuthOutcome.signedIn,
    'joined' => AuthOutcome.joined,
    'linked' => AuthOutcome.linked,
    'reconnected' => AuthOutcome.reconnected,
    'stepped_up' => AuthOutcome.steppedUp,
    'not_invited' => AuthOutcome.notInvited,
    'not_registered' => AuthOutcome.notRegistered,
    'invite_invalid' => AuthOutcome.inviteInvalid,
    'email_mismatch' => AuthOutcome.emailMismatch,
    'email_unverified' => AuthOutcome.emailUnverified,
    'mailbox_linked_elsewhere' => AuthOutcome.mailboxLinkedElsewhere,
    'step_up_wrong_account' => AuthOutcome.stepUpWrongAccount,
    'consent_blocked' => AuthOutcome.consentBlocked,
    'cancelled' => AuthOutcome.cancelled,
    'failed' => AuthOutcome.failed,
    _ => AuthOutcome.unknown,
  };
}
