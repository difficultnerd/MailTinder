# Mail Tinder: Plan Gaps and Open Questions

Last updated: 3 October 2026

Source: a gap review of `CONTEXT.md`, `mail-tinder-v1-user-stories.md`, the unsubscribe research brief and the `difficultnerd/MailTinder` template (Rust backend, Flutter app for iOS, Android and web). Two research threads own unsubscribe mechanics and cheap classification; this file covers everything else, and flags where their answers feed in.

Each item gives a recommended default. Once James confirms an item, move the decision into `CONTEXT.md` and strike it here.

## A. Decisions that block the build

Status: A1 to A6 decided on 3 October 2026 and recorded in `CONTEXT.md` under "Planning decisions". Kept below for the reasoning.

### A1. What happens to the email itself on each swipe

The stories define what each gesture does to the sender, not to the message.

| Gesture | Gap | Recommended default |
| --- | --- | --- |
| Left (reject) | Does the message stay in the inbox, get archived or go to trash? | Move to trash (recoverable), consistent with the v2 rule. |
| Right (keep) | Leave unread, mark read, or archive? | Mark read, leave in place. |
| Up (file) | Does filing remove it from the inbox? | Apply label or folder and archive out of the inbox. |
| Down (skip) | Does a skipped item come back, and when? | Returns to the Feed after the current backlog, never more than twice in a row. |

### A2. Left swipe on mail that has nothing to unsubscribe from

A personal email, a one-off receipt or a security alert has no list. Swiping left on mum's email must not fire an agent at a random link.

Recommended default: if the message has no List-Unsubscribe header and the classifier does not rate it as bulk mail, reject means "trash and block sender" (provider filter), with no agent dispatched.

### A3. Undo versus an unsubscribe already sent

An unsubscribe request cannot be recalled once sent. A persistent undo button implies it can.

Recommended default: hold every reject for a short grace window (about 10 seconds, or until the next swipe plus a few seconds) before firing the unsubscribe. Undo inside the window cancels cleanly. Undo after it restores the message and records that the unsubscribe was already sent.

### A4. What goes in the Feed

Unclear whether the Feed shows the whole inbox (personal mail included) or only bulk and marketing mail. This drives classification cost, the meaning of the spam score and how dangerous a careless left swipe is.

Recommended default: whole inbox, so the app is a full triage tool, with the card score relabelled from "spam confidence" to "bulk likelihood". Gmail and Outlook already filter real spam; what the user cares about is "is this a mailing list".

### A5. New mail only versus 20 years of backlog

`CONTEXT.md` says the app pulls new mail only, but the reason the app exists is working through 20 years of Gmail. Triaging old messages one at a time is not the v2 bulk feature; it is the core loop on older mail.

Recommended default: Feed shows new mail first; once caught up, it walks backwards through history a page at a time. v2 bulk reclassification stays deferred.

### A6. Platform for the trial

Superseded (3 October 2026, spec audit): no push and no daily digest in v1; Flutter web first is decided. See `CONTEXT.md`.

The template targets iOS, Android and web. Push notifications, distribution and Apple fees differ sharply.

Recommended default: Flutter web as an installable PWA first (no store review, one link for friends), with the daily digest sent by email. Add iOS through TestFlight once the loop is stable. iOS web push only works for PWAs added to the home screen, which is acceptable for a trial.

## B. Platform and policy constraints

### B1. Google restricted scopes

Superseded (3 October 2026, spec audit): v1 requests `gmail.send` for mailto unsubscribe and invites; scopes come from S8. The cost and Testing mode notes below still hold.

Reading and modifying Gmail (`gmail.modify`) is a restricted scope. Consequences:

- An unverified app in "Testing" mode is limited to 100 named test users. That covers James and 20 friends.
- In Testing mode, refresh tokens expire after 7 days, so friends must log in again weekly. Acceptable for a trial; poor for client demos.
- Publishing to production needs Google's OAuth verification plus an annual third-party security assessment (CASA). This is real cost and lead time, and should be a deliberate later decision.
- Cost as of late 2026: OAuth verification itself is free and takes about 4 to 6 weeks. CASA is paid to an authorised lab, renewed every 12 months, at a tier Google assigns. Tier 2 runs about USD 540 to 1,800 per year (TAC Security pricing); Tier 3 about USD 4,500. Lab turnaround is 1 to 4 weeks, plus remediation time. Sources: Google restricted scope verification help (support.google.com/cloud/answer/13465431), deepstrike.io and bright-softwares.com CASA cost articles.

Recommended default: stay in Testing mode for v1, request `gmail.modify` only (no `gmail.send`, no full `mail.google.com`), and treat production verification as a v2 gate.

- Planning assumption (James, 3 October 2026): expect the highest CASA tier (about USD 4,500 a year). Decided: build to OWASP ASVS Level 2 from day one (recorded in `CONTEXT.md`).

### B2. Microsoft Graph

Delegated `Mail.ReadWrite` plus `offline_access`, multi-tenant app registration that accepts personal Microsoft accounts. No equivalent of CASA, but users will see an "unverified publisher" prompt until publisher verification is done (needs a Microsoft Partner Network ID). Work or school accounts may be blocked by their tenant admin's consent policy. Outlook folders are exclusive, unlike Gmail labels, so filing behaves slightly differently per provider.

### B3. mailto unsubscribe needs a send scope

Many List-Unsubscribe headers only offer `mailto:`. Actioning those means sending mail as the user, which needs `gmail.send` or `Mail.Send`, another sensitive scope. The unsubscribe research thread will report how common this is. Product call needed: add the send scope, or route mailto-only cases to Needs Attention in v1.

### B4. Identity versus connected mailboxes

Login is OAuth through the mail provider, and Settings lets a user add several Gmail and Outlook accounts. Unspecified: which account is the identity, what happens if the user removes it, and whether two people can share a mailbox.

Recommended default: an internal user record created at first login; each mailbox is a linked account; any linked account can sign in; the last one cannot be removed without deleting the user.

### B5. Invite enforcement

Recommended default: an allowlist of email addresses James manages, checked at first OAuth callback. No invite codes in v1.

### B6. Merged or per-account Feed

Recommended default: one merged Feed with a small account badge on each card.

## C. Architecture questions that need an answer before code

Superseded (3 October 2026, spec audit): hosting is Google Cloud `us-central1` with Firestore (no Postgres, no Australian region); see `specs/S4-architecture.md` and `specs/S5-data-inventory.md`. Kept for the reasoning.

- **Mail ingestion.** Gmail push needs a Google Cloud Pub/Sub topic and a `watch` call renewed every 7 days; Outlook needs Graph change subscriptions renewed every few days. Both need a public HTTPS endpoint. Polling every few minutes is simpler for 20 users. Recommended default: poll in v1 behind the adapter, add push later.
- **Hosting.** Not decided. Needs a public endpoint, a database, a job queue for unsubscribe work and, depending on the research, a headless browser worker. Recommended default: one small container host plus managed Postgres in an Australian region; scale-to-zero is nice but not a v1 requirement.
- **What the server stores.** Superseded by `research/data-handling-and-in-account-ai.md` Part 1 (no mail content at rest). Original text kept for reference: Classification and the Feed need message content. Recommended default: store message IDs, headers, sender, subject and a short snippet; fetch full bodies from the provider on demand; never store attachments; encrypt OAuth refresh tokens at rest. This matches the proposed "treat mail as sensitive" principle in `CONTEXT.md`.
- **Data deletion.** Google's user data policy requires a way to delete stored data. Add "delete my account and data" to Settings in v1.
- **Audit log.** The proposed "every automated action is auditable" principle is what makes History, stats and undo possible. Recommend confirming it as decided; it shapes the data model from day one.

## D. Items owned by other threads

- **Unsubscribe research:** realistic success rate of the fallback agent, mailto prevalence, cost per attempt, whether the agent ships in v1. Answers A2, A3 and B3 in detail.
- **Classification research:** how bulk likelihood and filing suggestions are produced cheaply, and whether they share one model. Answers the last open question in `CONTEXT.md` and constrains A4.

## E. Smaller gaps

- **Trial success criteria.** v2 is gated on the core loop being "proven safe and reliable", with no measure defined. Suggest: unsubscribe success rate, undo rate on rejects (a proxy for mis-swipes) and zero unrecoverable actions over the friends trial.
- **Stats.** No definition of which stats Settings shows. Suggest: emails triaged, senders unsubscribed, unsubscribes confirmed working (no mail from that sender after 14 days).
- **Unsubscribe verification.** Nothing checks whether an unsubscribe worked. Watching for further mail from the sender is cheap and feeds the stats and the trial metric.
- **Name.** "Tinder" is a Match Group trademark. Fine for a private trial; worth a working name before client demos.
- **Super-like tab name.** Still open in `CONTEXT.md`. Suggest "Filed".
- **Accessibility.** Every gesture already has a button; add screen reader labels and reduced-motion support to the v1 definition of done.
