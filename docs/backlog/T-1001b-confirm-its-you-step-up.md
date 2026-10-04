# T-1001b: Confirm it's you (step-up overlay)

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M10 | sonnet | about 300 lines of code plus tests | T-1001a |

**Read only these spec sections:** S9 section 1.1 and the `step_up_wrong_account` row of "Copy for API outcomes" (`docs/specs/S9-functional-screens.md`); S7 sections 3.6, 4 (the `step_up_required` row) and 5.2 (API-AUTH-1 `step_up`, API-AUTH-3 `step_up_valid_until`) (`docs/specs/S7-api-contract.md`). Nothing else is needed.

## Goal

Any screen can wrap a sensitive call in `StepUpController.run`. When the server answers `403 step_up_required`, the app shows the Confirm it's you overlay, runs a fresh Google sign-in in a popup window, and once the server confirms the step-up it re-sends the waiting request without the user tapping again. T-1006a and T-1007a/b use it for add and disconnect mailbox, delete account and every admin write.

## Files

| Action | Path | What |
| --- | --- | --- |
| Create | `app/lib/state/step_up_controller.dart` | `StepUpController` |
| Create | `app/lib/screens/step_up/confirm_its_you_overlay.dart` | Overlay widget and `StepUpOverlayHost` |
| Change | `app/lib/main.dart` | Provide one `StepUpController`; wrap the app in `StepUpOverlayHost` through `MaterialApp.builder` |
| Change | `app/lib/screens/sign_in/auth_result_screen.dart` | Popup branch for `stepped_up`, `step_up_wrong_account`, `cancelled`, `failed` |
| Change | `app/lib/copy.dart` | Strings below |
| Create | `app/test/screens/step_up_test.dart` | Tests below |

## Types and signatures

```dart
typedef StepUpAction<T> = Future<T> Function();

enum StepUpStatus { idle, prompting, redirecting, popupBlocked }

class StepUpController extends ChangeNotifier {
  StepUpController({
    required ApiClient api,
    required SessionModel session,
    required Browser browser,
    DateTime Function()? now,                       // default DateTime.now; tests inject
    Duration pollEvery = const Duration(seconds: 2),
    Duration giveUpAfter = const Duration(minutes: 5),
  });
  StepUpStatus get status;
  String? get waitingActionLabel;                    // for example "to disconnect jane@example.com"

  /// Runs [action]. On ApiException code 'step_up_required' shows the overlay; after the session
  /// reports a fresh step-up, runs [action] exactly once more and returns its result.
  /// Returns null if the user cancels or confirmation does not arrive. Other errors are rethrown.
  Future<T?> run<T>({required String waitingActionLabel, required StepUpAction<T> action});

  /// Must be called synchronously from the button's onPressed (popup blockers).
  void continueWithGoogle();
  void cancel();
}

class StepUpOverlayHost extends StatelessWidget {
  const StepUpOverlayHost({super.key, required this.controller, required this.child});
}
```

## Algorithm

1. `run`: `try { return await action(); }`. Catch `ApiException` with `code == 'step_up_required'` only; rethrow everything else. Only one step-up runs at a time: a second `run` while one is open waits for the first to finish, then starts its own.
2. On step-up required: remember `baseline = session.session?.stepUpValidUntil`, set `waitingActionLabel`, status `prompting`, create a `Completer<bool>`, notify. The overlay (modal barrier plus centred panel) shows `Copy.confirmItsYou`, the waiting label, `Copy.stepUpExplainer`, "Continue with Google" and "Cancel".
3. `continueWithGoogle` (synchronous part): `popup = browser.openPopup('mt_step_up')`. If null, status `popupBlocked` and show `Copy.popupBlocked` in the panel; the button stays for a retry. Otherwise status `redirecting` (button disabled, spinner), then asynchronously `startAuth(intent: AuthIntent.stepUp)`, check it with `safeNavigationTarget`, and `popup.navigate(url)`. Any error: close the popup and finish as not confirmed (step 6).
4. Poll every `pollEvery` while redirecting: `await session.refresh()`; confirmed when `stepUpValidUntil != null`, it is after `now()`, and it differs from `baseline`. Polling the session, not messaging the popup, is the design `[DEFAULT]`: the full-page Google round trip would wipe the in-memory waiting request if done in the main tab, browser storage is not allowed (S5, ASVS V14.3.3), and the app has no `postMessage` listeners (ASVS V3.5.5).
5. Confirmed: complete with true, status `idle`, `popup.close()` (ignore errors), then re-run `action()` once. `session.refresh()` has already picked up the rotated CSRF token (the session ID rotates on step-up, S7 3.6). If the re-run again throws `step_up_required`, do not loop: finish as not confirmed.
6. Cancel, or `giveUpAfter` passed without confirmation: close the popup, status `idle`, show a SnackBar `Copy.stepUpNotConfirmed`, return null. The screen underneath is unchanged.
7. Popup side: the popup loads the app at `/#/auth/result?outcome=...` with `browser.isPopupWindow` true. In that case `AuthResultScreen` does not route; it shows a small page:
   - `stepped_up`: `Copy.stepUpConfirmedPopup`, then `browser.closeWindow()` after 1 second.
   - `step_up_wrong_account`: `Copy.stepUpWrongAccount` and a "Close" button.
   - `cancelled`, `failed` or anything else: `Copy.stepUpNotConfirmed` and a "Close" button.
   The main overlay keeps waiting after a wrong account, so the user can try again or cancel.

Copy (S9 verbatim unless marked):

| Constant | Text |
| --- | --- |
| `confirmItsYou` | Confirm it's you |
| `stepUpExplainer` | Google will ask you to sign in again. |
| `cancel` | Cancel |
| `stepUpWrongAccount` | That Google account isn't linked to your Mail Tinder account. Nothing was changed. |
| `stepUpNotConfirmed` | Not confirmed. Nothing was changed. |
| `stepUpConfirmedPopup` `[DEFAULT]` | Confirmed. You can close this window. |
| `popupBlocked` `[DEFAULT]` | Your browser blocked the sign-in window. Allow pop-ups for Mail Tinder, then try again. |
| `close` `[DEFAULT]` | Close |

Waiting-action labels are written by the calling tasks (for example `'to disconnect $address'`, S9 1.1).

## Acceptance criteria

| ID | Behaviour (one line) |
| --- | --- |
| XC-03 | The overlay's controls carry Semantics labels and meet tap target guidelines |

The AU-01 AC5, AU-04 AC6, AU-05 AC3, AU-06 AC3 and CL-04 AC8 tests are written by the tasks that own those actions (T-1006a, T-1007a, T-1007b) using this controller.

## Tests that must pass

- `'s9_step_up_prompt shows Confirm it\'s you and the waiting action'` (widget)
- `'s9_step_up_redirecting opens the popup and disables Continue'` (widget)
- `'s9_step_up_confirmed reruns the waiting action once without a second tap'` (widget, fake session flips `stepUpValidUntil`)
- `'s9_step_up_wrong_account popup page shows the not linked message'` (widget, `FakeBrowser.isPopupWindow` true)
- `'s9_step_up_cancelled shows Not confirmed and leaves the screen unchanged'` (widget)
- `'s9_step_up_timeout gives up after five minutes'` (widget, `tester.pump` with fake clock)
- `'s9_step_up_popup_blocked shows the pop-up message'` (widget)
- `'step-up start uses intent step_up and no invite token'` (widget)
- `'step-up runs the action directly when no step-up is needed'` (unit)
- `'step-up does not loop when the retried action asks again'` (unit)
- `'step-up rethrows errors other than step_up_required'` (unit)
- `'XC-03 Confirm it\'s you controls are labelled'` (widget)

## Edge cases and traps

- `openPopup` must run inside the tap handler before any `await`, or browsers block the popup.
- Never store the waiting action, its body or its idempotency key in browser storage; the closure lives in memory.
- Re-run the same closure, so a route that takes an `Idempotency-Key` gets the same one (S7 3.6).
- Do not add `window.onMessage` or `BroadcastChannel` listeners (ASVS V3.5.5).
- Use the injected `now`, not `DateTime.now()`, so the timeout test is deterministic.
- Treat a `stepUpValidUntil` equal to the baseline as not confirmed (an old step-up still in its window must not count as this one: in that case the server would not have asked).
- Do not keep polling after cancel or confirm; cancel the timer.

## Out of scope

- The screens that call `run` (T-1006a, T-1007a, T-1007b).
- Server-side step-up checks (T-504).

## Done when

- The tests above pass and every required check is green (S10 10.1).
- Definition of done in S10 10.4.
- No `postMessage` or storage API appears in `app/lib` (grep in the PR description).
