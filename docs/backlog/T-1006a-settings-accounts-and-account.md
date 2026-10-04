# T-1006a: Settings home, Connected accounts and Account

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M10 | sonnet | about 420 lines of code plus tests | T-1001b |

**Read only these spec sections:** S9 sections 7 (intro), 7.1, 7.5, 1.1 and section 9 (`docs/specs/S9-functional-screens.md`); S7 sections 3.4 (outcomes `linked`, `mailbox_linked_elsewhere`), 4 (`last_mailbox`, `app_folder_move_failed`), 5.2 (API-AUTH-1, API-AUTH-4), 5.3 (API-MBX-1, API-MBX-2) and 5.10 (API-ACCT-1) (`docs/specs/S7-api-contract.md`); S2 ST-03, AU-04, AU-05, AU-06, AU-07 AC2, GM-02 AC2; register rows V7.4.4, V7.5.2, V10.7.3, V14.3.1 in `docs/security/asvs-l2-register.md`. Nothing else is needed.

## Goal

The Settings tab gets its home list, the Connected accounts screen (list, add Gmail, sign in again, disconnect) and the Account screen (Sounds switch, Sign out, delete account). Sensitive actions go through `StepUpController` from T-1001b. Later tasks add their own entries to the Settings list.

## Files

| Action | Path | What |
| --- | --- | --- |
| Create | `app/lib/screens/settings/settings_screen.dart` | Home list built from `settingsEntries` |
| Create | `app/lib/screens/settings/settings_entries.dart` | `SettingsEntry` and the registry list |
| Create | `app/lib/screens/settings/connected_accounts_screen.dart` | S9 7.1; replaces T-1001a's `/settings/accounts` placeholder |
| Create | `app/lib/screens/settings/account_screen.dart` | S9 7.5 |
| Create | `app/lib/state/play_prefs.dart` | `PlayPrefs` (Sounds switch, memory only) |
| Change | `app/lib/api/api_client.dart`, `http_api_client.dart`, `fake_api_client.dart` | `listMailboxes`, `disconnectMailbox`, `deleteAccount` (and `signOut` if T-007 lacks it) |
| Change | `app/lib/routes.dart`, `app/lib/main.dart` | Routes `/settings/accounts`, `/settings/account`; provide `PlayPrefs` |
| Change | `app/lib/copy.dart` | Strings below |
| Create | `app/test/screens/settings_accounts_test.dart`, `app/test/screens/settings_account_test.dart` | Tests below |

## Types and signatures

```dart
class SettingsEntry {
  const SettingsEntry({required this.title, required this.route, this.adminOnly = false});
  final String title; final String route; final bool adminOnly;
}
// settings_entries.dart: this task adds Connected accounts and Account. T-1006b adds History, Rules, Stats;
// T-1007a adds Admin and Experiments; T-1007b adds Bake-off report. Display order follows S9 7.
final List<SettingsEntry> settingsEntries = [ /* ... */ ];

class DeleteAccountResult { final DateTime deletionDueBy; final List<({String mailboxId, String emailAddress})> appFoldersNotDeleted; }

abstract class ApiClient {
  Future<List<Mailbox>> listMailboxes();                 // GET /mailboxes
  Future<void> disconnectMailbox(String mailboxId);      // DELETE /mailboxes/{id}
  Future<DeleteAccountResult> deleteAccount();           // DELETE /account (202)
}

class PlayPrefs extends ChangeNotifier {   // memory only: resets to off in each new tab [DEFAULT]
  bool get soundsOn; set soundsOn(bool v);
}
```

## Algorithm

1. **Settings home:** `ListView` of `settingsEntries`, hiding `adminOnly` entries unless `session.user.isAdmin`.
2. **Connected accounts** (ST-03 AC1): `listMailboxes()`; each row shows "Gmail", the address and status text `Copy.statusConnected` or `Copy.statusNeedsSignIn`. Rows with `needs_sign_in` show "Sign in again": `startAuth(intent: reconnect, mailboxId)` then `browser.assign`.
3. **Route argument:** when opened with an `AuthOutcome` from T-1001a: `linked` shows SnackBar `Copy.mailboxAdded` `[DEFAULT]`; `mailboxLinkedElsewhere` shows `Copy.mailboxLinkedElsewhere` `[DEFAULT]`.
4. **Add Gmail** (AU-04 AC1, AC6): `stepUp.run(waitingActionLabel: Copy.stepUpAddGmail, action: () => api.startAuth(intent: AuthIntent.link))`; non-null result goes through `safeNavigationTarget` and `browser.assign`.
5. **Disconnect** (AU-05): if only one mailbox, show dialog `Copy.onlyMailbox` with "Go to Account" (AU-05 AC2) and send nothing. Otherwise confirm with `Copy.disconnectQuestion(address)`, then `stepUp.run(waitingActionLabel: Copy.stepUpDisconnect(address), action: () => api.disconnectMailbox(id))`. Success: reload the list and `session.refresh()`. `409 last_mailbox` shows the only-mailbox dialog. `409 app_folder_move_failed` shows `Copy.appFolderMoveFailed` and the mailbox stays listed (AU-05 AC4). Other errors `Copy.actionFailed`.
6. **Account:** a `SwitchListTile` "Sounds" bound to `PlayPrefs.soundsOn`, off at start (GM-02 AC2).
7. **Sign out** (AU-07 AC2, ASVS V14.3.1): call `api.signOut()` inside `try`; whatever happens, `session.wipe()` (T-007 clears every in-memory model) and replace the stack with Sign-in. A network failure must still wipe.
8. **Delete account** (AU-06 AC3): first dialog `Copy.deleteAccountExplain` with "Cancel" and "Continue"; second dialog `Copy.deleteAccountConfirm` with "Cancel" and "Delete account". Then `stepUp.run(waitingActionLabel: Copy.stepUpDeleteAccount, action: api.deleteAccount)`. On success: if `appFoldersNotDeleted` is not empty, show a dialog `Copy.appFoldersNotDeleted` listing each address; then wipe as in step 7 and show SnackBar `Copy.accountDeleted` `[DEFAULT]` on Sign-in.

Copy (S9 verbatim unless marked):

| Constant | Text |
| --- | --- |
| `settingsConnectedAccounts` | Connected accounts |
| `settingsAccount` | Account |
| `addGmail` | Add Gmail |
| `disconnect` | Disconnect |
| `statusConnected` `[DEFAULT]` | Connected |
| `statusNeedsSignIn` `[DEFAULT]` | Needs sign-in |
| `mailboxAdded` `[DEFAULT]` | Mailbox added. |
| `mailboxLinkedElsewhere` `[DEFAULT]` | That Google account is already linked to another Mail Tinder account. Nothing was linked. |
| `onlyMailbox` `[DEFAULT]` | This is your only mailbox. To remove it, delete your account instead. |
| `goToAccount` `[DEFAULT]` | Go to Account |
| `disconnectQuestion(a)` `[DEFAULT]` | Disconnect $a? Its cards leave your Feed and its queued unsubscribes are cancelled. |
| `appFolderMoveFailed` | Couldn't move your Mail Tinder data to another mailbox, so nothing was disconnected. Try again. |
| `stepUpAddGmail` `[DEFAULT]` | to add a Gmail account |
| `stepUpDisconnect(a)` | to disconnect $a |
| `stepUpDeleteAccount` `[DEFAULT]` | to delete your account |
| `sounds` | Sounds |
| `signOut` | Sign out |
| `deleteAccount` `[DEFAULT]` | Delete account |
| `deleteAccountExplain` `[DEFAULT]` | Delete your Mail Tinder account? We delete your Mail Tinder settings file from your Google Drive, cancel queued unsubscribes, disconnect your mailboxes and destroy your encryption key. Labels already on your messages stay in your mailbox. |
| `deleteAccountConfirm` `[DEFAULT]` | This can't be undone. Delete your account now? |
| `appFoldersNotDeleted` `[DEFAULT]` | We couldn't delete your Mail Tinder settings file from these mailboxes. Remove it in Google Drive, under Settings, Manage apps: |
| `accountDeleted` `[DEFAULT]` | Your account is deleted. |

## Acceptance criteria

| ID | Behaviour (one line) |
| --- | --- |
| ST-03 AC1 | Connected accounts lists every mailbox with provider, address and status |
| AU-04 AC1 | Add Gmail starts the `link` intent and navigates to Google |
| AU-04 AC2 | Three Gmail mailboxes are listed and told apart by address |
| AU-04 AC6 | Add Gmail without a fresh sign-in shows Confirm it's you |
| AU-05 AC2 | The only mailbox cannot be disconnected; the app points to delete account |
| AU-05 AC3 | Disconnect without a fresh sign-in shows Confirm it's you |
| AU-05 AC4 | `app_folder_move_failed` shows the S9 message and the mailbox stays |
| AU-06 AC3 | Delete account without a fresh sign-in shows Confirm it's you |
| AU-07 AC2 | Sign out ends the session and returns to Sign-in |
| GM-02 AC2 | The Sounds switch is off by default |
| ASVS V7.4.4 | Sign out is reachable from every signed-in tab |
| ASVS V7.5.2 | The user sees and ends their one session with Sign out |
| ASVS V10.7.3 | Settings lists linked mailboxes and Disconnect calls the API |
| ASVS V14.3.1 | Sign out wipes in-memory state even when the server cannot be reached |

## Tests that must pass

- `'ST-03 AC1 Connected accounts lists provider, address and status'` (widget)
- `'AU-04 AC1 Add Gmail starts the link intent and navigates to Google'` (widget)
- `'AU-04 AC2 three Gmail mailboxes are listed by address'` (widget)
- `'AU-04 AC6 Add Gmail without a fresh sign-in shows Confirm it\'s you'` (widget)
- `'AU-05 AC2 the only mailbox points to delete account and sends nothing'` (widget)
- `'AU-05 AC3 Disconnect without a fresh sign-in shows Confirm it\'s you'` (widget)
- `'AU-05 AC4 app_folder_move_failed shows the S9 message and keeps the mailbox'` (widget)
- `'AU-06 AC3 delete account without a fresh sign-in shows Confirm it\'s you'` (widget)
- `'AU-07 AC2 Sign out ends the session and returns to Sign-in'` (widget)
- `'GM-02 AC2 Sounds switch is off by default'` (widget)
- `'asvs_v7_4_4 sign out is reachable from every tab'` (widget: from each tab, tap Settings, Account, find Sign out)
- `'asvs_v7_5_2 Account shows Sign out for the one session'` (widget)
- `'asvs_v10_7_3 Settings lists linked mailboxes and Disconnect calls the API'` (widget)
- `'asvs_v14_3_1 sign out wipes memory when the server cannot be reached'` (widget, `NetworkException`)
- `'s9_connected_accounts_needs_sign_in offers Sign in again'` (widget)
- `'s9_connected_accounts_linked_elsewhere shows the message'` (widget)
- `'s9_account_delete two-step confirm explains labels stay'` (widget)
- `'s9_account_delete app folders not deleted lists the mailboxes'` (widget)
- `'s9_settings_admin_entries hidden for non-admins'` (widget)

## Edge cases and traps

- Every step-up action goes through `stepUp.run`; never call `disconnectMailbox` or `deleteAccount` directly from a button.
- The "Add Gmail" call is `startAuth` itself: the step-up is checked when the `link` start is posted, not at the callback.
- The Sounds preference lives in memory only (S5 Browser: nothing in browser storage); do not use `shared_preferences`.
- Sign out must wipe even when `signOut()` throws.
- Do not hide the only mailbox's Disconnect button silently: tapping it explains why (AU-05 AC2).
- "Add Microsoft" and the consent-blocked status are v2.

## Out of scope

- History, Rules, Stats (T-1006b); Admin, Experiments (T-1007a); Bake-off report (T-1007b).

## Done when

- The tests above pass and every required check is green (S10 10.1).
- Definition of done in S10 10.4.
