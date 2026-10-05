import 'package:app/api/models/auth.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  group('AuthIntentWire', () {
    test('wire maps every intent to its S7 string', () {
      expect(AuthIntent.signIn.wire, 'sign_in');
      expect(AuthIntent.join.wire, 'join');
      expect(AuthIntent.link.wire, 'link');
      expect(AuthIntent.reconnect.wire, 'reconnect');
      expect(AuthIntent.stepUp.wire, 'step_up');
    });
  });

  test('parseAuthOutcome maps every S7 outcome and unknown', () {
    expect(parseAuthOutcome('signed_in'), AuthOutcome.signedIn);
    expect(parseAuthOutcome('joined'), AuthOutcome.joined);
    expect(parseAuthOutcome('linked'), AuthOutcome.linked);
    expect(parseAuthOutcome('reconnected'), AuthOutcome.reconnected);
    expect(parseAuthOutcome('stepped_up'), AuthOutcome.steppedUp);
    expect(parseAuthOutcome('not_invited'), AuthOutcome.notInvited);
    expect(parseAuthOutcome('not_registered'), AuthOutcome.notRegistered);
    expect(parseAuthOutcome('invite_invalid'), AuthOutcome.inviteInvalid);
    expect(parseAuthOutcome('email_mismatch'), AuthOutcome.emailMismatch);
    expect(parseAuthOutcome('email_unverified'), AuthOutcome.emailUnverified);
    expect(
      parseAuthOutcome('mailbox_linked_elsewhere'),
      AuthOutcome.mailboxLinkedElsewhere,
    );
    expect(
      parseAuthOutcome('step_up_wrong_account'),
      AuthOutcome.stepUpWrongAccount,
    );
    expect(parseAuthOutcome('consent_blocked'), AuthOutcome.consentBlocked);
    expect(parseAuthOutcome('cancelled'), AuthOutcome.cancelled);
    expect(parseAuthOutcome('failed'), AuthOutcome.failed);
    // Anything else -> unknown
    expect(parseAuthOutcome('something_else'), AuthOutcome.unknown);
    expect(parseAuthOutcome(null), AuthOutcome.unknown);
    expect(parseAuthOutcome(''), AuthOutcome.unknown);
  });
}
