# T-1007a: Experiments and Admin screens

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M10 | sonnet | about 420 lines of code plus tests | T-1006a |

**Read only these spec sections:** S9 sections 7.6, 7.8, 1.1 and 9 (`docs/specs/S9-functional-screens.md`); S7 sections 3.6, 4 (`consent_outdated`, `experiment_unavailable`, `forbidden`), 5.11 (API-ADM-1 to API-ADM-7, API-ADM-15, API-ADM-16) and 5.13 (API-EXP-1, API-EXP-2) (`docs/specs/S7-api-contract.md`); S2 AU-01, AU-02 AC2 and AC3, AU-07 AC5, CL-02; S10 9.2 row BAKE-7. Nothing else is needed.

## Goal

Every user gets Settings, Experiments: one off-by-default switch with the exact consent text. Admins get Settings, Admin: invites, invite requests and users, with every write behind Confirm it's you.

## Files

| Action | Path | What |
| --- | --- | --- |
| Create | `app/lib/consent_text.dart` | `kExperimentsConsentText`, `kExperimentsConsentVersion` |
| Create | `app/lib/api/models/experiments.dart` | `MyExperiments` |
| Create | `app/lib/api/models/admin.dart` | `Invite`, `InviteStatus`, `InviteRequest`, `AdminUser`, pages |
| Create | `app/lib/state/experiments_model.dart`, `admin_model.dart` | View models |
| Create | `app/lib/screens/settings/experiments_screen.dart` | S9 7.8 |
| Create | `app/lib/screens/admin/admin_screen.dart` | Tabs: Invites, Requests, Users |
| Change | `app/lib/screens/settings/settings_entries.dart` | Admin (`adminOnly`), Experiments |
| Change | `app/lib/api/api_client.dart`, `http_api_client.dart`, `fake_api_client.dart` | Methods below |
| Change | `app/lib/copy.dart` | Strings below |
| Create | `app/test/fixtures/consent_expectations.dart` | Expected phrases for BAKE-7 (one place, so a region change is a one-line edit) |
| Create | `app/test/screens/experiments_test.dart`, `admin_test.dart` | Tests below |

## Types and signatures

```dart
const String kExperimentsConsentVersion = '2026-10-03';
const String kExperimentsConsentText = 'Try an experimental classifier. Card text (sender, subject and the first part of the message) is sent to Google (Vertex AI, United States) and TypeSafe AI (United States) to compare two classifiers. Neither trains on it. TypeSafe has not yet committed to how long it keeps this data and has no data processing agreement or security attestation in place. Anonymous accuracy figures from your swipes may be published. Anonymous totals already published or saved stay as they are if you later opt out.';

class MyExperiments { final bool available, optedIn; final String? consentVersion; final String currentConsentVersion; final DateTime? optedInAt; }

enum InviteStatus { pending, used, revoked, expired }
class Invite { final String inviteId, emailAddress; final InviteStatus status; final DateTime createdAt, expiresAt, lastSentAt; }
class InviteRequest { final String requestId, emailAddress; final DateTime createdAt; }
class AdminUser { final String userId, emailAddress; final DateTime createdAt; final bool isAdmin, signedIn; final int mailboxCount; final DateTime? lastSeenAt; }
class Paged<T> { final List<T> items; final String? nextCursor; }

abstract class ApiClient {
  Future<MyExperiments> getMyExperiments();                                       // GET /me/experiments
  Future<MyExperiments> putMyExperiments({required bool optedIn, required String consentVersion}); // PUT
  Future<Paged<Invite>> listInvites({String? cursor});                            // ADM-1
  Future<Invite> createInvite(String emailAddress);                               // ADM-2 (201 or 200)
  Future<Invite> resendInvite(String inviteId);                                   // ADM-3
  Future<void> revokeInvite(String inviteId);                                     // ADM-4
  Future<Paged<InviteRequest>> listInviteRequests({String? cursor});              // ADM-5
  Future<Invite> approveInviteRequest(String requestId);                          // ADM-6
  Future<void> declineInviteRequest(String requestId);                            // ADM-7
  Future<Paged<AdminUser>> listUsers({String? cursor});                           // ADM-16
  Future<void> endUserSession(String userId);                                     // ADM-15
}
```

## Algorithm

1. **Experiments:** load `getMyExperiments()`. Show `kExperimentsConsentText` in full above a switch "Experiments" `[DEFAULT label]`. The switch shows on only when `optedIn && consentVersion == currentConsentVersion`.
2. Turning on: if `currentConsentVersion != kExperimentsConsentVersion`, the app's text is stale: show `Copy.consentChanged` and do not send (the app build must be updated). Else `putMyExperiments(optedIn: true, consentVersion: kExperimentsConsentVersion)`. `409 consent_outdated`: reload and show `Copy.consentChanged`. `409 experiment_unavailable` or `available == false`: switch disabled and `Copy.experimentPaused` shown.
3. Turning off (CL-02 AC3): confirm dialog `Copy.experimentsOffQuestion` with "Cancel" and "Turn off"; then `putMyExperiments(optedIn: false, consentVersion: kExperimentsConsentVersion)`. Opting out always proceeds, even when `available` is false.
4. **Admin** route guard: if `!session.user.isAdmin` show `Copy.adminsOnly` `[DEFAULT]` and make no admin calls. A `403 forbidden` anywhere shows `Copy.actionFailed`.
5. **Invites tab:** a field "Email address" and button "Invite". Validate: trimmed, one `@`, non-empty both sides, at most 320 characters `[DEFAULT]`; invalid shows `Copy.emailInvalid`. Then `stepUp.run(waitingActionLabel: Copy.stepUpInvite(address), action: () => api.createInvite(address))`; refresh the list. Rows: address, status word, `formatDate(lastSentAt)` as sent date. Pending or expired rows: "Re-send" (step-up, `Copy.stepUpResend(address)`); pending rows: "Revoke" (confirm, step-up, `Copy.stepUpRevoke(address)`).
6. **Requests tab:** rows address and `formatDateTime(createdAt)`; "Approve" (step-up `Copy.stepUpApprove(address)`) and "Decline" (confirm, step-up `Copy.stepUpDecline(address)`). Remove the row on success.
7. **Users tab:** rows address and `Copy.mailboxCount(n)`; "End session" enabled when `signedIn`, confirm `Copy.endSessionQuestion(address)`, then step-up `Copy.stepUpEndSession(address)` and `endUserSession(userId)`.
8. All three lists page with `nextCursor` (load more at the end).

Copy (S9 verbatim unless marked): consent text as above (S9 7.8, verbatim); `consentChanged` `[DEFAULT]` "The consent text has changed. Read it again before you switch this on."; `experimentPaused` "The experiment is paused" (S7 section 4); `experimentsOffQuestion` `[DEFAULT]` "Turn this off? Your experiment records will be deleted. Anonymous totals already published or saved stay as they are."; `adminsOnly` `[DEFAULT]` "Admins only."; `emailInvalid` `[DEFAULT]` "Enter a valid email address."; `stepUpInvite(a)` `[DEFAULT]` "to invite $a"; `stepUpResend(a)` `[DEFAULT]` "to re-send the invite to $a"; `stepUpRevoke(a)` `[DEFAULT]` "to revoke the invite to $a"; `stepUpApprove(a)` `[DEFAULT]` "to approve $a"; `stepUpDecline(a)` `[DEFAULT]` "to decline $a"; `stepUpEndSession(a)` `[DEFAULT]` "to end $a's session"; `endSessionQuestion(a)` `[DEFAULT]` "End $a's session? They'll need to sign in again."; `mailboxCount(n)` `[DEFAULT]` "$n mailboxes" ("1 mailbox" singular); invite status words `[DEFAULT]` "Pending", "Used", "Revoked", "Expired"; buttons "Invite", "Re-send", "Revoke", "Approve", "Decline", "End session".

## Acceptance criteria

| ID | Behaviour (one line) |
| --- | --- |
| CL-02 AC1 | Experiments shows an off-by-default switch with the full consent text |
| BAKE-7 | The consent text names Google (Vertex AI, United States), TypeSafe AI (United States), what is sent, no training, the missing retention, DPA and attestation, publication, and that saved totals stay (S10 test `bake_7_consent_text_names_recipients`) |
| CL-02 AC3 | Turning off asks to confirm deletion and sends `opted_in: false` |
| AU-01 AC1 | An admin invites by email address |
| AU-01 AC2 | Re-send calls the resend endpoint |
| AU-01 AC4 | Revoke calls the revoke endpoint |
| AU-01 AC5 | An admin write without a fresh sign-in shows Confirm it's you |
| AU-02 AC2 | The requests list shows address and request time with Approve and Decline |
| AU-02 AC3 | Approve and Decline call their endpoints |
| AU-07 AC5 | End session confirms, then calls the admin endpoint |

## Tests that must pass

- `'CL-02 AC1 Experiments switch is off by default with the consent text'` (widget)
- `'BAKE-7 consent text names recipients'` (widget, phrases from `consent_expectations.dart`)
- `'CL-02 AC3 turning off asks to confirm deletion and sends opted_in false'` (widget)
- `'AU-01 AC1 admin invites by email'` (widget)
- `'AU-01 AC2 re-send calls resend'` (widget)
- `'AU-01 AC4 revoke calls revoke after confirming'` (widget)
- `'AU-01 AC5 an admin write without a fresh sign-in shows Confirm it\'s you'` (widget)
- `'AU-02 AC2 requests list shows address and time with approve and decline'` (widget)
- `'AU-02 AC3 approve and decline call their endpoints'` (widget)
- `'AU-07 AC5 End session confirms then calls the admin endpoint'` (widget)
- `'s9_experiments_on shows the switch on'` (widget)
- `'s9_experiments_paused disables the switch'` (widget)
- `'s9_experiments_consent_outdated shows the text again'` (widget)
- `'s9_admin_hidden_for_non_admins'` (widget)
- `'s9_admin_invalid_email is refused before any call'` (widget)
- `'XC-03 Experiments and Admin controls are labelled'` (widget)

## Edge cases and traps

- The consent text is copied exactly from S9 7.8, including straight apostrophes; do not reflow or edit it.
- Never send `opted_in: true` with any version other than `kExperimentsConsentVersion`.
- Every admin write goes through `stepUp.run`; reads do not.
- Hiding Admin for non-admins is cosmetic; the server enforces `admin` (do not rely on the hide).
- Addresses in admin lists are plain `Text`; do not log them.

## Out of scope

- Bake-off report, kill switches and snapshots (T-1007b).

## Done when

- The tests above pass and every required check is green (S10 10.1).
- Definition of done in S10 10.4.
