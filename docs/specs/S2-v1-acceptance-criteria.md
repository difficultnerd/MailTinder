# S2: Version 1 Stories and Acceptance Criteria

Status: DRAFT for James's review. 3 October 2026.
Revised 3 October 2026 (spec audit): findings H1, H3, H4, H5, M1, M2, M5, M6, M7, M19, L3 and L10 of `docs/reviews/spec-audit.md` applied. Story and AC IDs are unchanged; new ACs are added at the end of their story.
Revised 3 October 2026 (spec audit, James's answers to Q1 to Q4): v1 is Gmail only and Microsoft moves to v2 (H8); items marked "v2" are not built in v1, but the domain and adapter trait stay provider-neutral so Microsoft needs only a new adapter. The page handler moves to v2; mailto stays in v1 (M18). The app folder is one primary folder per user (H7). The Experiments consent text names TypeSafe's missing retention terms (M17).
Revised 3 October 2026 (James): the passkey lock is dropped for the trial and moves to the v2 hardening backlog. Sign-in is Google OAuth only, one session per user, step-up is a fresh Google sign-in, and one per-user `data_key` (Cloud KMS) protects stored tokens and the app folder. Passkey ACs keep their IDs, marked "Removed".
Sources: `docs/CONTEXT.md` (Planning decisions), `docs/mail-tinder-v1-user-stories.md`, research in `research/`.

## How to read this

- Each story has an ID (for example `SW-03`). Task files and tests reference these IDs.
- Each acceptance criterion (AC) is written as Given, When, Then, so it maps to one or more automated tests. A test name should include the AC ID, for example `sw_03_ac2_reject_trashes_message`.
- `[ASSUMES Dn]` marks a criterion that rests on a default in `docs/planning-roadmap.md` that James has not yet confirmed. Change the criterion if the decision changes.
- `[TUNABLE]` marks a number chosen as a starting value. It lives in configuration, not code.
- Terms: **mailbox** is one connected Gmail account (Microsoft accounts arrive in v2). **App folder** is the user's one primary app folder, in the Drive of their first linked Gmail mailbox; other mailboxes' Drives are not used. **User** is one person, who may have many mailboxes. **Card** is one message shown in the Feed. **Sort rule** is a learned rule about a sender or list. **Job** is queued background work.

## Glossary of fixed values

| Name | Starting value | Notes |
| --- | --- | --- |
| `UNSUB_DELAY` | 5 minutes `[TUNABLE]` | Delay before a queued unsubscribe is sent (James: "a few minutes") |
| `PERSONAL_BLOCK_THRESHOLD` | 3 rejects in 90 days `[TUNABLE]` | James: "a few times" |
| `SKIP_MAX_RETURNS` | 2 | A skipped card returns at most this many times |
| `NEEDS_ATTENTION_TTL` | 30 days | From data handling research |
| `JOB_TTL` | 1 hour | Hard limit on any queued job record. There is no `awaiting_session` state (passkey lock dropped for the trial, James 3 October 2026) |
| `INVITE_TTL` | 7 days `[TUNABLE]` | Life of an invite token (AU-01 AC1). Decided 3 October 2026 (spec audit) |
| `STEP_UP_WINDOW` | 5 minutes | A sensitive action needs a fresh Google sign-in within this window with one of my linked Google accounts (step-up: `prompt=login`, `max_age=300`, `auth_time` checked). Decided 3 October 2026 (spec audit, revised when the passkey lock was dropped) |

---

## 1. Access and accounts (AC prefix `AU`)

### AU-01 Invite a person by email

As James (admin), I want to invite a person by email address, so that only people I choose can use the app.

- **AC1.** Given I am an admin, when I enter a valid email address and confirm, then an invite record exists for that address and the invitee receives an email with a sign-in link. The link carries a single-use invite token: 256 bits of randomness, stored only as a hash, expiring after `INVITE_TTL`.
- **AC2.** Given an invite already exists for an address, when I invite the same address again, then no duplicate is created and the invite is re-sent with a new token; the old token stops working.
- **AC3.** Given I am not an admin, when I call the invite endpoint, then the request is refused with 403 and the attempt is recorded in the security log.
- **AC4.** Given an invite, when I revoke it before it is used, then a later sign-in with that address is refused.
- **AC5.** Every admin write (invite, re-send, revoke, approve or decline a request, end a user's session, bake-off kill switches, save or delete a snapshot) needs step-up: a fresh Google sign-in within `STEP_UP_WINDOW`. Without it the request is refused and the app asks me to sign in with Google again.

### AU-02 Request an invite

As a person without an invite, I want to request one, so that James can let me in.

- **AC1.** Given I am not invited, when I sign in with Google (Microsoft in v2), then I see a "Request an invite" screen and no mailbox access token is kept after the request is submitted.
- **AC2.** Given I submit a request, then James sees it in an admin list with the email address and request time, and can approve or decline it.
- **AC3.** Given James approves, then the request becomes an invite (AU-01 AC1 applies). Given James declines, then the requester is not told why and the request is deleted.
- **AC4.** Given more than 5 requests from one IP address in an hour `[TUNABLE]`, when another arrives, then it is refused with 429.

### AU-03 Sign in with an invited account

As an invited person, I want to sign in with my Google account (Microsoft in v2), so that I don't need a password.

- **AC1.** Given an invite for address X, when I open the invite link and complete Google OAuth (authorisation code with PKCE, `state` and `nonce`, ID token validated) with an account whose email is X and whose `email_verified` is true, and the invite token is valid, then a user record is created, the mailbox is linked, the token is used up, and I land on the Feed.
- **AC2.** Given an invite for address X, when I complete OAuth with an account whose email is not X and that account is not linked to any user, then access is refused and no token is kept.
- **AC3.** Given the provider reports the email as unverified, then access is refused. v2 Microsoft rule: the email counts as verified only when the `xms_edov` claim is present and true.
- **AC4.** Given I am an existing user, when I complete Google OAuth with any Gmail mailbox linked to my user (matched by Google `sub`), then I reach my own Feed. No invite token is needed. Account recovery is Google's own; the app has no recovery flow.
- **AC4a.** Removed (passkey lock dropped for the trial, James 3 October 2026).
- **AC4b.** Removed (passkey lock dropped for the trial, James 3 October 2026).
- **AC4c.** Removed (passkey lock dropped for the trial, James 3 October 2026).
- **AC5.** Sessions use a server-issued cookie named `__session` with HttpOnly, Secure and SameSite=Lax (Lax is required by the OAuth redirect); session IDs rotate at sign-in; idle timeout 15 minutes and absolute timeout 12 hours (ASVS V7). A user has one session: a new sign-in ends the old one (AU-07).
- **AC6.** Given the invite token is missing, already used, expired, revoked or replaced by a re-send, when I complete OAuth, then access is refused even if the email matches, and no token is kept. Redemption needs both the token and the email match.
- **AC7.** A mailbox is identified by the provider's stable account ID, never by its email address: Google `sub`; for Microsoft in v2, `tid` plus `oid`.

### AU-04 Connect more mailboxes

As a user, I want to connect several mailboxes, including several from the same provider, so that I can triage them together.

- **AC1.** Given I am signed in, when I add a Gmail mailbox (Microsoft in v2) from Settings and complete OAuth, then it is linked to my user and its mail appears in the Feed.
- **AC2.** Given I already have two Gmail mailboxes linked, when I link a third, then all three are linked and distinguishable by address in Settings.
- **AC3.** Given a mailbox is already linked to a different user, when I try to link it, then it is refused.
- **AC4.** v2. Given a Microsoft work tenant blocks consent, when the provider returns a consent error, then I see a plain message that my organisation's admin must approve the app, and nothing is linked. `[ASSUMES D17]`
- **AC5.** Only the scopes listed in the provider adapter spec (S8) are requested.
- **AC6.** Linking a mailbox needs step-up: a fresh Google sign-in within `STEP_UP_WINDOW`. Without it nothing is linked and the app asks me to sign in with Google again.

### AU-05 Disconnect a mailbox

- **AC1.** Given I have more than one mailbox, when I disconnect one, then its tokens are revoked at the provider (where the provider supports revocation), deleted from our store, its cards leave the Feed, and its queued jobs are cancelled.
- **AC2.** Given I have exactly one mailbox, when I try to disconnect it, then I am told to delete my account instead.
- **AC3.** Disconnecting a mailbox needs step-up: a fresh Google sign-in within `STEP_UP_WINDOW`. Without it nothing changes and the app asks me to sign in with Google again.
- **AC4.** Given I disconnect my primary mailbox, then before its tokens are revoked the app folder file is copied to the Drive app folder of my next linked mailbox, which becomes primary, and the old copy is deleted. If the copy fails, nothing is disconnected and I am told to try again. `[default, 3 October 2026]`

### AU-06 Delete my account

- **AC1.** Given I confirm deletion, then within the deletion request, in this order: my app folder file is deleted from my primary app folder, my queued jobs and their Cloud Tasks are cancelled, and my provider tokens are revoked. Then my per-user key (`data_key`) is destroyed (crypto-shredding), and within 24 hours every remaining server record for my user is swept.
- **AC2.** Labels or categories already applied to my messages remain, since they are my mail.
- **AC3.** Deleting my account needs step-up: a fresh Google sign-in within `STEP_UP_WINDOW`. Without it nothing is deleted and the app asks me to sign in with Google again.

### AU-07 Sign out and one session

As a user, I want to sign out, and to know that only my latest sign-in is active, so that a forgotten browser does not stay signed in.

- **AC1.** A user has one session. Given I sign in while another session of mine exists, then the old session ends on the server and its next request is refused with 401.
- **AC2.** When I choose "Sign out" in Settings, then my session ends on the server and the browser is told to clear the app's stored data and cache.
- **AC3.** Removed (session list and "Sign out everywhere" dropped with the passkey lock, James 3 October 2026; one session per user makes them unnecessary).
- **AC4.** Removed (passkey lock dropped for the trial, James 3 October 2026).
- **AC5.** Given I am an admin, when I end a user's session, then that user's session ends and a security event is logged. This is an admin write, so AU-01 AC5 applies.
- **AC6.** Signing in, signing out, a session ended by a new sign-in and an admin ending a session are each recorded as a security event.

---

## 2. Feed (AC prefix `FD`)

### FD-01 One card at a time

- **AC1.** Given I have mail, when I open the Feed, then exactly one card is in focus, showing sender name and address, subject, a body preview, the mailbox badge, and the bulk badge.
- **AC2.** The body preview is plain text, at most 300 characters `[TUNABLE]`, with HTML stripped and no remote content loaded (no tracking pixels).
- **AC3.** No message content is written to the server database, logs or caches while building the card. (Verified by test against the data inventory, S5.)

### FD-02 Merged Feed across mailboxes

- **AC1.** Given I have several mailboxes, then the Feed interleaves their mail by received time, newest first.
- **AC2.** Each card shows which mailbox it came from.
- **AC3.** Given one mailbox's provider call fails, then cards from other mailboxes still load and a banner names the failing mailbox.

### FD-03 New mail first, then backlog

- **AC1.** Given unseen mail newer than my last session, then it appears before any older mail.
- **AC2.** Given I have triaged all new mail, then the Feed continues with older mail, newest to oldest, a page at a time.
- **AC3.** Given I leave and return, then the Feed resumes at the same position (cursor stored in my app folder).
- **AC4.** There is no daily cap.
- **AC5.** New mail is fetched when the app opens and on pull to refresh.

### FD-04 Mail already handled elsewhere

- **AC1.** Given a message was trashed, archived or read-and-filed in another client since it was fetched, when its card would be shown, then it is skipped silently and not shown.

---

## 3. Swipes (AC prefix `SW`)

Every gesture has an equivalent button with the same behaviour and an accessible label.

### SW-01 Right: keep

- **AC1.** Given a card, when I swipe right, then the message stays in its mailbox location, unchanged, and the next card appears.
- **AC2.** The keep is counted toward keep-learning for that sender (FL-04).

### SW-02 Down: skip

- **AC1.** Given a card, when I swipe down, then nothing changes in the mailbox and the next card appears.
- **AC2.** A skipped card returns later in the queue, either at the end of the current page or at a random later position drawn from an injected random source (seedable in tests) `[TUNABLE]`, at most `SKIP_MAX_RETURNS` times, then is not shown again until a new session.

### SW-03 Left: reject

- **AC1.** Given a card, when I swipe left, then the message moves to the provider's trash (never permanent delete).
- **AC2.** Given the sender is classed as a mailing list with an unsubscribe mechanism, then an unsubscribe job is queued with due time now plus `UNSUB_DELAY`, and a rejected-sender sort rule is created (SR-01). In v1 the mechanism is one-click or mailto; an https link without one-click raises a Needs Attention item instead (UN-04 AC6).
- **AC3.** Given the message is classed as suspected spam or phishing, then it is reported to the provider as spam and no unsubscribe is queued.
- **AC4.** Given the sender is classed as personal, then only AC1 applies, and the reject is counted for PB-01.
- **AC5.** Given the message has no List-Unsubscribe header, then no unsubscribe attempt of any kind is made (no body-link parsing, no page handler); AC1 applies, and a reject_list rule is created for `bulk_no_header` mail only; a `notice` is trash only (S3 message classes). Spike E1: every headerless sender sampled was an account, billing or security notice or a relay.

### SW-04 Up: super-like and file

- **AC1.** Given a card, when I swipe up, then a filing sheet shows the suggested category first plus up to two alternates and "New category".
- **AC2.** When I confirm a category, then the provider label (Gmail) or category (Outlook, v2) is applied and the message leaves the inbox.
- **AC3.** Given the app has no suggestion, then I am asked to name a new category (FL-02).

### SW-05 Undo

- **AC1.** Given I have made at least one swipe this session, when I tap Undo, then the last swipe is fully reversed: the message returns to its previous location and labels, and the card returns to the Feed.
- **AC2.** Given the last swipe queued an unsubscribe still before its due time, when I undo, then the job is cancelled and the rejected-sender rule is removed.
- **AC3.** Given the unsubscribe was already sent, when I undo, then the message is restored, the rule is removed, and I am told the unsubscribe request had already gone.
- **AC4.** Undo reverses one swipe per tap and works back through every swipe made in the current session.
- **AC4a.** Given the last swipe reported a message as spam, when I undo, then the message is restored to its previous location, and I am told the spam report itself cannot be recalled.
- **AC5.** Given an undo arrives at the moment the unsubscribe falls due, then exactly one outcome occurs: either the cancel wins and no request is sent (AC2), or the send wins and undo reports it already went (AC3). Never both, never neither. Enforced by a conditional state transition on the job record.

---

## 4. Unsubscribe (AC prefix `UN`)

### UN-01 Batched delayed unsubscribe

- **AC1.** Given a queued unsubscribe job, when its due time passes, then it runs exactly once.
- **AC2.** Given several jobs for the same list, then one unsubscribe request is sent.
- **AC3.** Every job outcome (sent, failed, cancelled, expired) is recorded in the security log (pseudonymous) and stored on the job record; the next time I load the Feed, the api appends it to History in my app folder. The `unsub` service never writes the app folder and needs no Drive access.
- **AC4.** When a job runs and needs the mailbox (mailto send, Sent label), it mints a fresh access token from the stored refresh token (encrypted under my `data_key`). No access token is stored on the job. A job due 5 minutes later runs even if I have closed the app. Queued unsubscribe jobs are the only background mailbox work.
- **AC5.** One-click POST (and the page handler in v2) needs no mailbox token and always runs at the due time. Mailto sends and the Sent label use the access token minted for the job.
- **AC6.** Given the refresh token is revoked or invalid when the job runs, then the job ends `failed` and a Needs Attention item says "Sign in again" for that mailbox. It never fails silently and never touches my mailbox without a valid token.

### UN-02 One-click (RFC 8058)

- **AC1.** Given `List-Unsubscribe` has an https URI and `List-Unsubscribe-Post: List-Unsubscribe=One-Click`, and a passing DKIM signature covers both headers, then the job sends one HTTPS POST with body `List-Unsubscribe=One-Click` and no cookies or credentials.
- **AC2.** Given no passing DKIM signature covers both headers, then no unsubscribe job of any method is created; the message class is `bulk_no_header`, so it is trashed and a reject rule is created (CL-01 AC2). The same DKIM rule applies to every method: one-click, mailto (UN-03 AC3), the v1 "Open unsubscribe page" item (UN-04 AC6) and the v2 page handler (UN-04 AC5).
- **AC2a.** The check verifies the DKIM signature over the unsubscribe headers only; a DMARC pass is not required (spike E1 found legitimate senders failing DMARC).
- **AC3.** A 2xx response marks the job sent; any other response (except a 3xx, AC4) or timeout retries up to 3 times with backoff, then falls to UN-05.
- **AC4.** The POST follows no redirects: a 3xx response raises a Needs Attention item and is not retried. Before connecting, the job resolves the host and refuses a target that resolves to a private, loopback, link-local, CGNAT or metadata address (including 169.254.169.254 and `metadata.google.internal`): no request is sent and a Needs Attention item is raised. The connection uses the resolved address (pinned against DNS rebinding), https only, a 10-second timeout, no cookies or credentials and the fixed body. These controls run on the `unsub` service in v1 (S6 section 6).

### UN-03 Mailto

- **AC1.** Given only a `mailto:` URI, then the job sends the unsubscribe email from the same mailbox, using the address, subject and body in the URI.
- **AC2.** The sent unsubscribe email is left in Sent with a "Mail Tinder" label so it is identifiable; it is never deleted (INV-5). The label mechanism is defined in S8.
- **AC3.** Given no passing DKIM signature covers `List-Unsubscribe`, then no mailto job is created and no mail is sent from my mailbox; the message is `bulk_no_header` (CL-01 AC2).

### UN-04 Page handler (no one-click): v2

The page handler (headless Chromium service) is deferred to v2 with the open-ended agent (James, 3 October 2026, spec audit Q3). AC1 to AC5 are v2. In v1 only AC6 applies.

- **AC1.** v2. Given a List-Unsubscribe header with an https link but no one-click, then the link is opened in an isolated headless browser with no access to internal networks (SSRF controls in S6).
- **AC2.** v2. Given the page matches a known single-step pattern, then the handler completes it and classifies the result page as success, failure or unclear.
- **AC3.** v2. Given failure or unclear, then the job falls to UN-05.
- **AC4.** v2. The open-ended agent is not used in v1 or in the first v2 page handler.
- **AC5.** v2. Given no passing DKIM signature covers `List-Unsubscribe`, then no page handler job is created; the message is `bulk_no_header` (CL-01 AC2).
- **AC6.** v1. Given a reject on a message whose `List-Unsubscribe` has an https link but no one-click, and a passing DKIM signature covers `List-Unsubscribe`, then no unsubscribe job is created; the message is trashed, a reject rule is created (SR-01), and a Needs Attention item is raised with an "Open unsubscribe page" action. The link comes only from the DKIM-covered header, never from the body. Without that DKIM cover the message is `bulk_no_header` and no item is raised.

### UN-05 Needs Attention

- **AC1.** Given a job cannot complete, then a Needs Attention item is created with the sender, the link, the reason it stopped, and an "Open in browser" action.
- **AC2.** No job loops: every job ends in sent, failed-to-Needs-Attention, cancelled or expired within `JOB_TTL`. Cloud Tasks (`maxAttempts` 4) is the only retry layer.

### UN-06 Delivery check

- **AC1.** Given an unsubscribe was sent, when mail from the same list arrives more than 5 business days later, then that mail is trashed by the rejected-sender rule and a Needs Attention item says the unsubscribe did not take effect.
- **AC2.** This check and the Stats measure in ST-02 AC1 both apply and are independent: this check raises Needs Attention for mail arriving more than 5 business days after sending; ST-02 counts an unsubscribe as "confirmed working" only after 14 days with no new mail from the list.

---

## 5. Sort rules and blocking (AC prefix `SR`, `PB`)

### SR-01 Rejected-sender rule

- **AC1.** Given a reject on a list message, then a rule is created keyed on sender address plus List-Id (or sender address alone when no List-Id exists).
- **AC1a.** Given the message arrived through a known forwarding relay (for example iCloud Hide My Email), then the rule key uses the original sender recovered from the relay format, not the relay address.
- **AC2.** Given a new message matches the rule, then it is moved to trash when fetched and recorded in History, and never shown in the Feed.
- **AC3.** Given a rule keyed on sender address plus List-Id, a message from the same address with a different or no List-Id does not match. Given a rule keyed on sender address alone (no List-Id), it matches only messages that carry `List-Unsubscribe`, and where the rejected message carried `Feedback-ID`, only messages with the same `Feedback-ID`. Receipts and other mail from that address without `List-Unsubscribe` are never matched.
- **AC4.** Given I switch a rule off in History, then it stops matching from then on.
- **AC5.** Rules are stored in my app folder, not on our server.
- **AC6.** Given the rejected message has neither a DKIM signature aligned with its From domain nor a provider authentication pass, then no reject rule is created and nothing is counted; the message is still trashed (SW-03 AC1). Spoofed mail never creates a rule.

### PB-01 Block a person after repeated rejects

- **AC1.** Given I have already rejected personal mail from the same sender `PERSONAL_BLOCK_THRESHOLD` minus one times, when I reject it again (the third time with the default), then I am asked "Block <name>?" with Block preselected.
- **AC2.** When I confirm, then a block rule is created that trashes future mail from that address (SR-01 AC2 behaviour) and is listed in History.
- **AC3.** When I decline, then I am not asked again for that sender for 90 days `[TUNABLE]`.
- **AC4.** Only rejects of messages with a DKIM signature aligned with the From domain, or a provider authentication pass, count toward `PERSONAL_BLOCK_THRESHOLD`. Other rejects are not counted and never raise a block prompt, so spoofed mail "from" a friend cannot get the friend blocked.

---

## 6. Filing and keep-learning (AC prefix `FL`)

### FL-01 Suggest a category

- **AC1.** Given I have filed mail from this sender before, then the suggestion is the category I used most for that sender.
- **AC2.** Given no sender history, then the suggestion comes from the classification layer (header rules, then on-device model where available).
- **AC3.** The suggestion and alternates appear within 200 ms of the swipe, p95, measured in current stable desktop Chrome on the reference machine (James's own laptop, to be recorded in S10) with sender history only; the server part (suggestion endpoint) is automated-tested at p95 100 ms or less.

### FL-02 Name a new category

- **AC1.** Given I choose "New category", when I enter a name, then a label or category with that name is created in the mailbox (if absent) and applied.
- **AC2.** Given the on-device model is available, then a proposed name is pre-filled and editable.

### FL-03 Suggestions get quieter, never silent

- **AC1.** Given I have confirmed the same suggestion for a sender 3 times `[TUNABLE]`, then the filing sheet shows a one-tap confirm with alternates collapsed.
- **AC2.** Filing never happens without a tap.

### FL-04 Keep-learning

- **AC1.** Given I have kept mail from a sender 5 times `[TUNABLE]` and never rejected it, then the next card from that sender shows a quiet prompt suggesting a category.
- **AC2.** Accepting creates a filing sort rule shown in History; ignoring it has no effect.

### FL-05 Filed screen

- **AC1.** The Filed tab lists my categories with message counts and opens a category to show its messages.

---

## 7. Needs Attention and notifications (AC prefix `NA`)

### NA-01 Needs Attention screen

- **AC1.** Lists every open item, newest first, with sender, reason and actions (open link, mark done, dismiss).
- **AC2.** Items are deleted on resolve or after `NEEDS_ATTENTION_TTL`.

### NA-02 Notifications

Removed from v1 (James, 3 October 2026): no push notifications and no daily digest. The Needs Attention tab badge shows the open item count (S9).

---

## 8. Settings, history and stats (AC prefix `ST`)

### ST-01 History

- **AC1.** Lists every automated action (trash by rule, unsubscribe outcome, filing, block) with time, sender and mailbox, newest first.
- **AC2.** Each rule-driven entry links to its rule, which can be switched off.

### ST-02 Stats

- **AC1.** Shows emails triaged, senders unsubscribed, and unsubscribes confirmed working (no mail from the list 14 days after sending). The UN-06 delivery check also applies; see UN-06 AC2.
- **AC2.** Shows the mail stopped total ("About 3,200 emails a year stopped"), the sum of the per-rule yearly estimates (GM-05).
- **AC3.** Shows the achievements list with unlocked ones dated and locked ones greyed (GM-06).

### ST-03 Connected accounts

- **AC1.** Lists every linked mailbox with provider, address and status (connected, needs sign-in; consent blocked is v2, Microsoft only).

---

## 9. Classifier bake-off (AC prefix `CL`)

Design in S4 section 5 (CR-01, revised).

### CL-01 Pluggable classifier with header guard

- **AC1.** Every classification comes through the `Classifier` trait and then the header guard; adding or swapping an implementation needs no change outside the classifier module.
- **AC2.** Given a message without a `List-Unsubscribe` header covered by a passing DKIM signature (plus `List-Unsubscribe-Post` for one-click), then whatever any classifier says, its class is not `list` and no unsubscribe job of any method is created. A message that carries the header without that DKIM cover is `bulk_no_header`: trash and reject rule, no job. DMARC is not required.
- **AC3.** During the bake-off the card badge comes from header rules only.

### CL-02 Experiments setting

- **AC1.** Settings, Experiments shows an off-by-default switch with the consent text (Google Vertex AI in the United States and TypeSafe AI in the United States, what is sent, no training, that TypeSafe has not yet committed to how long it keeps this data and has no data processing agreement or security attestation in place, possible publication of anonymous accuracy figures, and that anonymous totals already published or saved stay as they are after opt-out).
- **AC2.** Given I have not consented, no request about my mail reaches Gemini or Jev (test JEV-1).
- **AC3.** Given I turn it off, model calls stop immediately and my `classifier_eval` records are deleted.

### CL-03 Head-to-head scoring

- **AC1.** Given I have consented, Gemini and Jev both classify each card in parallel with identical input.
- **AC2.** Given a model errors, times out (2 seconds) or returns invalid output, the failure is recorded for that model and the card is unaffected.
- **AC3.** When I swipe, one `classifier_eval` record is written with both predictions, the header-rules result, my action, undo status and time to swipe, and no sender, subject, text or message ID.
- **AC4.** The swipe handler accepts only a valid sealed classification token for that message and re-applies the header guard.

### CL-04 Admin controls and report

- **AC1.** An admin can switch Gemini or Jev off without a deploy (config document, checked every minute).
- **AC2.** An admin report shows, for header rules, Gemini and Jev: accuracy against swipes, per-class confusion, calibration, precision and recall for junk, the false-junk rate, agreement between methods, latency and cost per 1,000 messages, updated as swipes arrive. Aggregates only.
- **AC3.** Every rate in the report carries a 95% interval, and the report includes a paired comparison of the models on the same cards (McNemar's test and a participant-resampled interval on the accuracy difference), the participant count and the largest contributor's share.
- **AC4.** The report breaks results down by header-rules class, header facts, provider, message age, text length and language, hiding any segment below the minimum cell size.
- **AC5.** Given records with different question or input versions, the report does not pool them unless the admin asks.
- **AC6.** An admin can save the report as a snapshot that holds no pseudonymous IDs, survives the 180-day expiry and opt-outs, and stays until the admin deletes it.
- **AC7.** The report and any snapshot download as one CSV table with the same figures and suppression as the on-screen report.
- **AC8.** Kill switches, saving a snapshot and deleting a snapshot are admin writes and need step-up (AU-01 AC5).

---

## 10. Progress and play (AC prefix `GM`)

Design goal (James): the user feels they are making progress. None of these features adds a Firestore collection, a log field, browser storage or a provider scope.

### GM-01 Inbox meter

- **AC1.** The Feed shows the total inbox count across all mailboxes and the change since the session started (for example "12,431, down 214 today").
- **AC2.** The count is fetched from the provider's folder totals when the app opens and after every 10 swipes `[TUNABLE]`; one mailbox failing shows the total for the rest with a marker.

### GM-02 Swipe feedback

- **AC1.** Each direction has its own animation; a reject on a three-poop card plays the "flush" effect.
- **AC2.** Sounds are off by default and can be switched on in Settings; haptics are used only where the browser supports the Vibration API.
- **AC3.** A combo counter appears after 5 swipes within 10 seconds `[TUNABLE]`; confetti plays at every 100 cleared in a session.
- **AC4.** All effects respect the reduced-motion setting (XC-03).

### GM-03 End-of-round card

- **AC1.** Given I leave the Feed, reach 50 swipes since the last card `[TUNABLE]`, or reach the "up to date" divider, then a card shows cleared, kept, filed, senders unsubscribed and senders blocked for the round.
- **AC2.** Round totals live in browser memory only and are lost when the tab closes.

### GM-04 Backlog years as levels

- **AC1.** Once new mail is cleared, the Feed shows the current level as a calendar year with mail left (for example "Level 2023: 1,840 left"), counted by one provider date-range query.
- **AC2.** Given the last message of that year leaves the inbox, then a level-complete screen shows and the next older year begins.
- **AC3.** The current level comes from the Feed cursor in the app folder; nothing new is stored.

### GM-05 Mail stopped counter

- **AC1.** Given a reject creates a reject_list rule or an unsubscribe, then in the same request the api counts the sender's matching messages over the past 90 days (one provider count query) and stores the yearly rate as one integer on the rule.
- **AC2.** History shows the per-sender figure; Stats shows the total (ST-02 AC2).
- **AC3.** If the count query fails, the rule is still created and the figure shows as unknown.

### GM-06 Achievements

- **AC1.** A fixed list unlocks locally: first unsubscribe, 100 senders silenced, a year cleared, 1,000 cleared, first filing category, first blocked person, ten unsubscribes in a round.
- **AC2.** Only the achievement ID and unlock date are stored, in the app folder.
- **AC3.** An unlock shows a short celebration on the Feed and appears in Stats.

### GM-07 Blitz mode

- **AC1.** From the Feed I can start a 60-second round; a timer and score are visible, and the round ends at zero with a results card.
- **AC2.** Cards with a personal badge are excluded from blitz rounds.
- **AC3.** The unsubscribe delay and undo work exactly as outside blitz.
- **AC4.** Block prompts (PB-01) are held until the round ends, then shown in turn.
- **AC5.** No blitz data is stored beyond the round's results card in memory.

### GM-08 Boss senders

- **AC1.** A sender becomes a boss when the app's sender stats show at least 20 messages seen `[TUNABLE]` and the sender is among the user's top 5 by count; no metadata scan is run.
- **AC2.** When a boss's card appears, a banner shows the sender with a health bar of inbox mail left from that sender, from one provider count query.
- **AC3.** Rejecting a boss "defeats" it: a short celebration plays and the boss leaves the boss list.

---

## 11. Cross-cutting (AC prefix `XC`)

- **XC-01.** No message bodies, subjects, snippets, addresses or URLs appear in application logs, error reports or analytics. Verified by a log-scanning test and the privacy Semgrep rules.
- **XC-02.** Every provider call goes through the adapter trait; core code has no Gmail or Graph types. The provider enum and mailbox key stay generic, so adding Microsoft in v2 (or another provider) needs a new adapter, not refactoring.
- **XC-03.** Every gesture has a button, every control has a screen reader label, and animations respect reduced-motion settings.
- **XC-04.** Every failure the user can act on surfaces as a visible message or Needs Attention item; nothing fails silently.
- **XC-05.** All ASVS Level 2 requirements mapped in S6 have a passing verification before release.
