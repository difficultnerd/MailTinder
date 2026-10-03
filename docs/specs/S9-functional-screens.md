# S9: Functional Screen Spec

Status: DRAFT for James's review. 3 October 2026.
Revised 3 October 2026 (spec audit): findings H2, M1 (step-up), L3, L4 and L7 of `docs/reviews/spec-audit.md` applied. Existing section numbers are kept because S7 cites them; new screens take new numbers. Also applied (James's answers to Q1 to Q4): v1 is Gmail only, so every Microsoft element is marked v2 (H8); the page handler is v2 and an https-only unsubscribe link becomes a Needs Attention item (M18); the Experiments consent text names TypeSafe's missing retention terms (M17).
Revised 3 October 2026 (James): the passkey lock is dropped for the trial. Sign-in is Google only; the passkey, recovery and Security screens are removed; step-up (section 1.1) is a fresh Google sign-in; "Sign out" is in Settings, Account (section 7.5), with one session per user.
Scope: what each screen shows, what each control does, and every state a screen can be in. Visual design (colours, icons, animation style, the badge artwork) is deliberately out of scope and comes later.
Depends on: `S2-v1-acceptance-criteria.md` (story IDs in brackets), `S3-domain-model.md`.

## Screen map

```
Sign-in ──> Request invite (if not invited)
   │
   └──> Bottom tabs: Feed | Filed | Needs Attention | Settings
                       │                               ├── Connected accounts
                       │                               ├── History
                       │                               ├── Rules
                       │                               ├── Stats
                       │                               ├── Account (sign out, sounds, delete)
                       │                               ├── Admin (admins only): invites, requests, users
                       │                               ├── Bake-off report (admins only)
                       │                               └── Experiments
                       ├── Blitz round
                       └── Filing sheet (overlay), Block prompt (overlay), Undo toast

Confirm it's you (overlay on any screen): fresh Google sign-in for step-up
```

Each screen below lists **content**, **controls** and **states**. Every state needs a test.

## 1. Sign-in

- **Content:** product name, one-line description, "Continue with Google" ("Continue with Microsoft" is added in v2), link to privacy notice.
- **Controls:** "Continue with Google" starts Google OAuth. When the person arrived through an invite link, the invite token travels with the OAuth request. An existing user signs in with any linked Gmail mailbox. [AU-03 AC1, AC4]
- **States:**
  - Default.
  - Arrived from an invite link: "You've been invited. Continue with the Google account the invite was sent to." [AU-01 AC1, AU-03 AC1]
  - Redirecting (spinner, button disabled).
  - Not invited: goes to Request invite. [AU-02]
  - Invite link expired, used or replaced: "This invite link no longer works. Ask for a new invite." [AU-03 AC6]
  - Email mismatch: "This account isn't the one you were invited with. Try the invited account, or ask for a new invite." [AU-03 AC2]
  - Email not verified: "We couldn't confirm this account's email address. Try another account." [AU-03 AC3]
  - v2 only. Consent blocked by work tenant: "Your organisation needs to approve Mail Tinder before you can connect this account." [AU-04 AC4]
  - Provider error or cancelled: "Sign-in didn't finish. Try again." with retry.
  - Signed out, or signed out because the same account signed in somewhere else: the default state, after the browser's stored data for the app has been cleared (section 7.5). [AU-07 AC1]

### 1.1 Confirm it's you (overlay) [AU-01 AC5, AU-04 AC6, AU-05 AC3, AU-06 AC3]

Step-up. Shown over the current screen when an action needs a fresh Google sign-in and the last one was more than 5 minutes ago (`STEP_UP_WINDOW`): delete account, add or disconnect a mailbox, and every admin change (including kill switches and snapshots).

- **Content:** "Confirm it's you" and the action waiting, for example "to disconnect jane@example.com". "Google will ask you to sign in again."
- **Controls:** "Continue with Google" (Google sign-in that always asks for the account again), "Cancel".
- **States:**
  - Redirecting.
  - Confirmed: back on the same screen, and the waiting action carries on without the user repeating it.
  - Signed in with an account not linked to this user: "That Google account isn't linked to your Mail Tinder account. Nothing was changed."
  - Cancelled or failed: "Not confirmed. Nothing was changed." The overlay closes and the screen is as before.

## 2. Request invite

- **Content:** the address the person signed in with, short explanation that the app is invite only.
- **Controls:** "Request an invite", "Use a different account".
- **States:** default; submitted ("Request sent. You'll get an email if it's approved."); rate limited ("Too many requests. Try again later."). [AU-02]

## 3. Feed

The main screen. One card in focus, the next card partly visible behind it.

- **Card content:** sender name, sender address, subject, body preview (plain text), mailbox badge (which account), bulk badge (score and reason on tap). [FD-01, FD-02]
- **Gestures and buttons (always both):**

| Gesture | Button | Result | Story |
| --- | --- | --- | --- |
| Right | Keep | Card leaves; message unchanged | SW-01 |
| Left | Reject | Card leaves; outcome depends on message class (S3) | SW-03 |
| Up | File | Filing sheet opens | SW-04 |
| Down | Skip | Card leaves; returns later in the queue, at most twice | SW-02 |
| None | Undo (persistent) | Last swipe reversed; card returns | SW-05 |

- **Feedback after each swipe:** a short toast naming what happened, for example "Trashed. Unsubscribing in 5 minutes." or "Kept." The toast includes Undo. For a reject that queues an unsubscribe, the toast states the delay.
- **Bulk badge tap:** small panel with the reason, for example "Mailing list, sent through Mailchimp, has one-click unsubscribe". [S2 card badge]
- **Progress elements (GM-01, GM-04, GM-08):** inbox meter at the top (total and change today); level banner once working through older mail ("Level 2023: 1,840 left"); boss banner with health bar when a boss sender's card is shown.
- **Blitz entry (GM-07):** a "Blitz" button starts a 60-second round (section 3.1).
- **Swipe feedback (GM-02):** per-direction animation, optional sound (off by default, switched on in Settings, Account, section 7.5), combo counter, confetti every 100 cleared; reduced-motion respected.
- **Overlays:**
  - End-of-round card: cleared, kept, filed, unsubscribed, blocked. [GM-03]
  - Level complete: the year cleared and the next year unlocked. [GM-04]
  - Achievement unlocked. [GM-06]
  - Boss defeated. [GM-08]
  - Filing sheet (section 4).
  - Block prompt: "You've rejected <name> 3 times. Block them?" with Block (default) and Not now. [PB-01]
  - Keep-learning prompt: inline on the card, quiet: "You always keep these. File under <category>?" with File and dismiss. [FL-04]
- **States:**
  - Loading first cards.
  - Normal.
  - Caught up on new mail, moving to older mail: one-time divider card "You're up to date. Now working back through older mail." Shown when the API reports `phase_changed` (S7). [FD-03 AC2]
  - Empty (no mail at all in any mailbox): "Nothing to triage."
  - One mailbox failing: banner "Can't reach <address>. Sign in again" with action; other cards still load. [FD-02 AC3]
  - All mailboxes need sign-in: full-screen prompt to sign in again.
  - Offline: banner; swipes disabled until back online.
  - Action failed (provider refused the change, API `502` or `503`): card returns with "Couldn't do that. Try again." and nothing is recorded as done. [XC-04]
- Message changed elsewhere (API `409 message_changed`): the card is dropped silently and the next card shows. [FD-04]
- **Undo states:** disabled when no swipe this session; enabled otherwise. After undoing a reject whose unsubscribe already went: "Restored. The unsubscribe request had already been sent." [SW-05 AC3]

### 3.1 Blitz round [GM-07]

- **Content:** the Feed with a 60-second timer and a score; personal-badge cards skipped.
- **Controls:** the usual gestures and buttons, Undo, and End round.
- **States:** running; ended (results card, then any held block prompts); interrupted (tab hidden pauses the timer).

## 4. Filing sheet

Opens on an up-swipe.

- **Content:** suggested category first, up to two alternates, "New category". Once a sender is learned, a single "File under <category>" button with "Other" to expand. [FL-01, FL-03]
- **Controls:** tap a category to file; "New category" opens a name field (pre-filled with a proposed name when the on-device model is available). [FL-02]
- **States:** loading suggestion (must resolve within 200 ms target, otherwise show alternates without a suggestion); no categories yet (goes straight to name field); name already exists (selects the existing one); cancel (card returns to the Feed, nothing changes).

## 5. Filed tab

- **Content:** list of categories with message counts across all mailboxes. Tap opens a category list: sender, subject, date, mailbox badge. Tap a message opens it in the provider's own web client. [FL-05]
- **Controls:** rename category, delete category (removes the label only, never the messages, with confirm).
- **States:** empty ("Swipe up on an email to start filing."), loading, provider error per mailbox.

## 6. Needs Attention tab

- **Content:** items newest first. Each shows sender, mailbox badge, reason in plain words (for example "This sender needs you to unsubscribe on their page", "Mail still arriving after unsubscribe"), and when it was raised. [NA-01]
- **https-only unsubscribe (v1):** a list whose unsubscribe header has only a web link (no one-click) always raises an item here; "Open unsubscribe page" uses the link from the checked header only, never from the message body. [UN-04 AC6]
- **Controls per item:** "Open unsubscribe page" (opens in the user's browser), "Done", "Dismiss".
- **Mailbox signed out (v1):** when an unsubscribe job finds a mailbox's Google access revoked, the job stops and the item reads "Couldn't unsubscribe from <sender>. Sign in again to <address>." with a "Sign in again" action. [UN-01 AC6]
- **Badge on the tab:** count of open items.
- **States:** empty ("Nothing needs you."), loading.

## 7. Settings

### 7.1 Connected accounts [ST-03, AU-04, AU-05]

- **Content:** each mailbox with provider, address and status (connected, needs sign-in; consent blocked is v2, Microsoft only).
- **Controls:** "Add Gmail" ("Add Microsoft" is v2), per-mailbox "Sign in again" and "Disconnect" (confirm; blocked for the last mailbox with a pointer to delete account). Adding and disconnecting need Confirm it's you (section 1.1). [AU-04 AC6, AU-05 AC3]

### 7.2 History [ST-01]

- **Content:** every automated action, newest first: time, mailbox, sender, action, outcome. Filters: all, unsubscribes, rule actions, filing. Unsubscribe outcomes appear from the next Feed load after the job finishes. [UN-01 AC3]
- **Controls:** tap an entry linked to a rule to open the rule.

### 7.3 Rules [SR-01, PB-01, FL-04]

- **Content:** reject-list rules, blocked people, filing rules. Each shows what it matches and how many messages it has acted on.
- **Controls:** switch on or off; delete (confirm).

### 7.4 Stats [ST-02]

- **Content:** emails triaged, senders unsubscribed, unsubscribes confirmed working, mail stopped per year, and the achievements list (unlocked with dates, locked greyed). Visual design later.

### 7.5 Account [AU-06, AU-07, GM-02]

- **Content:** a "Sounds" switch, off by default. [GM-02 AC2]
- **Controls:** "Sign out"; delete account (two-step confirm, explains what is deleted and that labels stay in the mailbox; needs Confirm it's you, section 1.1). [AU-06 AC3]
- **Sign-out:** [AU-07 AC2] ends the session on the server; the response tells the browser to clear the app's stored data and cache (`Clear-Site-Data: "cache", "storage"`, S7), and the app wipes what it holds in memory. The app does the same on any `401`. Then the Sign-in screen shows.

### 7.6 Admin (admins only) [AU-01, AU-02, AU-07 AC5]

- **Content:** invites (address, status, sent date), invite requests (address, time) and users (address, number of mailboxes).
- **Controls:** invite by email, re-send (sends a new link; the old one stops working), revoke; approve or decline requests; per user, "End session" (confirm). Every one of these needs Confirm it's you (section 1.1). [AU-01 AC5]

### 7.7 Bake-off report (admins only) [CL-04]

- **Content:** the bake-off report (S4 5.6): per-method figures with intervals, the paired comparison, segments, the latency histogram and the daily trend. Suppressed cells show "Too few to show". A list of saved snapshots (name, date, card count).
- **Controls:** filters (date range, versions); kill switches for Gemini and Jev; "Save snapshot"; "Download CSV" for the live report or a snapshot; delete snapshot (confirm). Kill switches, saving and deleting need Confirm it's you (section 1.1). [CL-04 AC8]
- **States:** loading; not enough data yet; versions mixed (prompt to pick one version); snapshot saved.

### 7.8 Experiments [CL-02]

- **Content:** one switch, off by default: "Try an experimental classifier. Card text (sender, subject and the first part of the message) is sent to Google (Vertex AI, United States) and TypeSafe AI (United States) to compare two classifiers. Neither trains on it. TypeSafe has not yet committed to how long it keeps this data and has no data processing agreement or security attestation in place. Anonymous accuracy figures from your swipes may be published. Anonymous totals already published or saved stay as they are if you later opt out."
- **States:** off; on; turning off confirms that experiment records will be deleted.

## 8. Notifications (outside the app)

None in v1: no push and no digest. Gamification is decided in S2 section 10 (GM-01 to GM-08) and appears inside the app: the Feed (section 3), the Blitz round (section 3.1), Stats (section 7.4) and the Sounds switch (section 7.5).

## 9. Copy rules

- Australian English, plain words, no em or en dashes.
- Emojis are not used in copy. Playful elements (the badge scale) are drawn as icons in the visual design phase.
- Every error message says what happened and what the user can do next.


### Copy for API outcomes (S7)

| Outcome or code | Copy |
| --- | --- |
| `not_registered` (Google account not linked to any user, no invite) | "There's no Mail Tinder account for this Google account. Use your invite link, or request an invite." |
| `invite_invalid` | "This invite link no longer works. Ask for a new invite." |
| `step_up_wrong_account` | "That Google account isn't linked to your Mail Tinder account. Nothing was changed." |
| `trashed_unsubscribe_manual` toast | "Trashed. The unsubscribe link is in Needs Attention." |
| Needs Attention `https_only_unsubscribe` | "This sender needs you to unsubscribe on their website." with "Open unsubscribe page" |
| Needs Attention `one_click_redirect` | "The unsubscribe request was redirected, so we stopped. Open the page to finish." with "Open unsubscribe page" |
| Needs Attention `one_click_address_refused` | "We couldn't safely send this unsubscribe request. Check the sender's own unsubscribe options." |
| `409 app_folder_move_failed` | "Couldn't move your Mail Tinder data to another mailbox, so nothing was disconnected. Try again." |
## Decided

- **Filing tab name.** "Filed" (James, 3 October 2026, for now).
- **Reject toast.** States the delay honestly, for example "Trashed. Unsubscribing in 5 minutes." (James, 3 October 2026).
- **Google sign-in, no passkeys in v1.** Users sign in with Google only (Microsoft from v2); there are no passkey, recovery or session-list screens. One session per user; "Sign out" sits in Settings, Account (section 7.5). The passkey lock moves to the v2 hardening backlog (James, 3 October 2026).
