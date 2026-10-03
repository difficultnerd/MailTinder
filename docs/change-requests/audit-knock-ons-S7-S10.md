# Spec audit knock-ons for S7 and S10

**Superseded in part (3 October 2026, 23:04):** James dropped the passkey lock for the trial. Every passkey, PRF, `token_key`, session custody, recovery, `locked` state and `awaiting_session` item below is superseded: sign-in is Google OAuth only, one KMS-wrapped `data_key` per user, one session per user, and step-up is a fresh Google sign-in with a linked account. S7 and the product plan specs reflect this.

From the product plan thread, 3 October 2026. The product plan side of the spec audit (`docs/reviews/spec-audit.md`) and James's answers to Q1 to Q4 are applied in CONTEXT, the roadmap, S2 to S6, S9, the ASVS register and CR-01. These are the changes S7 (API contract) and S10 (test strategy) need to match. Section references point to the updated product plan specs.

## Decisions the API and tests must follow

- **Provider scope (Q2):** Gmail only in v1. Microsoft (Graph, OneDrive, Microsoft OAuth, `xms_edov`, `tid` plus `oid`) is v2. Provider enum and mailbox key stay generic.
- **Unsubscribe methods (Q3):** one-click and mailto in v1. The page handler is v2. An https-only List-Unsubscribe (DKIM-covered, no one-click) gives trash, a reject rule and a Needs Attention item with an "Open unsubscribe page" link taken from the header only.
- **Jev consent (Q4):** full consent text: "Try an experimental classifier. Card text (sender, subject and the first part of the message) is sent to Google (Vertex AI, United States) and TypeSafe AI (United States) to compare two classifiers. Neither trains on it. TypeSafe has not yet committed to how long it keeps this data and has no data processing agreement or security attestation in place. Anonymous accuracy figures from your swipes may be published. Anonymous totals already published or saved stay as they are if you later opt out."
- **D9 and D10 (Q1):** confirmed.

## S7 (API contract)

1. **Keys and sealed tokens (H1, M3; S6 section 5).** Two per-user keys: `token_key` (refresh tokens only; PRF plus KMS, or KMS only without PRF) and `data_key` (KMS, server-usable: app folder, sealed tokens, encrypted fields, job access tokens). Session custody: `token_key` wrapped under HKDF(raw session ID) on the session record. All opaque tokens (cursors, undo, classification, prompt refs) are AES-256-GCM under `data_key`, with associated data = token type, user ID, `session_record_id` (stable across rotation, separate from `session_hash`) and expiry. Type and expiry are in clear and authenticated; the route checks the expected type. Section 2 "Opaque tokens" should say this.
2. **Passkey endpoints (H2).** `/auth/passkey/register/{options,verify}`, `/auth/passkey/assert/{options,verify}`, `/me/passkeys` (list, add, delete; the last passkey cannot be deleted). `userVerification: "required"`. A first-time session becomes authenticated only after passkey registration. Define the `locked` session state (S9 1.3 "Confirm it's you" doubles as Unlock). Resuming `awaiting_session` jobs at sign-in is a resume with a new Cloud Task, not a retry (S3).
3. **OAuth is recovery only for existing users (H3).** Separate intents: `join` (invite redemption) and `recover`. `join` by an existing user returns an "already registered" outcome that changes nothing. `recover` deletes every stored refresh token whatever the PRF state, ends all sessions, logs a security event, then requires a new passkey and re-linking. `pre_auth` expires after 10 minutes `[TUNABLE]`.
4. **Invite token (H4).** Single-use, 256-bit, stored hashed, 7 days `[TUNABLE]` (`INVITE_TTL`), required at redemption with the email match. Re-send issues a new token and voids the old. Error codes: invite token invalid or expired, email not verified, already registered.
5. **Step-up (M1).** Passkey assertion with user verification within 5 minutes (`STEP_UP_WINDOW`) for: delete account, link or disconnect a mailbox, add or remove a passkey, ending another session, every admin write (including kill switches and snapshots). A `step_up_required` error with the waiting action resumable after the check. State whether a fresh passkey registration counts as step-up (recommended: yes, so re-linking after recovery does not prompt again).
6. **Sessions (M2).** `GET /me/sessions`, `DELETE /me/sessions/{id}`, "sign out everywhere", admin `DELETE /admin/users/{id}/sessions`, and an admin user list (S9 7.6). At most 10 concurrent sessions per user `[TUNABLE]`; a new sign-in beyond that ends the oldest and logs a security event.
7. **Deletion order (M7).** In the request: delete the app folder file, cancel jobs and Cloud Tasks, revoke tokens; then crypto-shred both keys and sweep within 24 hours.
8. **Disconnecting the primary mailbox.** The app folder file moves to the next linked mailbox's Drive first, which becomes primary; if the move fails nothing is disconnected (S2 AU-05 AC4). `Mailbox` may expose `is_primary`.
9. **CSRF (M13).** S6 and the register now adopt S7's design (synchroniser token plus `Origin` check, on top of SameSite=Lax). No change needed in S7.
10. **CSV export filename (register V5.4.1).** ADM-10 and ADM-12 must use a server-fixed ASCII filename, not one built from query parameters.
11. **Unsubscribe outcomes.** `UnsubscribeJob.method` is `one_click` or `mailto` in v1. New Needs Attention reason codes: https-only header (with the "Open unsubscribe page" link), one-click 3xx (no redirects followed, no retry), one-click target refused by the address checks. `captcha` and `page_*` codes are v2. No Needs Attention item for body links (M4, already S7's).
12. **Config document.** ADM-9 names `config/classifiers` for the kill switches.
13. **`Clear-Site-Data` (L7).** Sign-out responds with `Clear-Site-Data: "cache", "storage"`.
14. **v2 labels.** Microsoft scopes and endpoints, consent-blocked states and the ST-03 status values for Microsoft are labelled v2.

## S10 (test strategy)

1. **WebAuthn end to end:** Chrome DevTools virtual authenticator with PRF on and off; user verification off must be refused.
2. **Recovery:** deletes every refresh token whatever the PRF state, ends all sessions, logs a security event; `join` by an existing user changes nothing.
3. **Invites:** token single use, expiry, replacement on re-send, refusal when the email matches but the token is missing or stale.
4. **Step-up:** required for each listed action; expires after 5 minutes.
5. **Sessions:** list, end one, sign out everywhere, admin end-all, the 10-session cap.
6. **Sealed tokens:** a token of one type is rejected as another; the undo stack survives session ID rotation; session custody unwrap works on any instance.
7. **DKIM rule (H5):** one rule for every method; failing it gives `bulk_no_header` with no job. The two corpus rows that send a DKIM failure to the page handler change to "trash plus rule, no job".
8. **One-click SSRF on `unsub` (v1):** refuses private, loopback, link-local, CGNAT and metadata addresses (including `metadata.google.internal`); pins the resolved IP against DNS rebinding; follows no redirects; a 3xx raises Needs Attention with no retry.
9. **`HttpEgress` allowlist** per service, with a test.
10. **Rules (M5, M6):** a sender-only reject rule matches only mail with `List-Unsubscribe` and the same `Feedback-ID` where present; receipts survive; mail without a DKIM-aligned From or provider authentication pass creates no rule and adds no block count.
11. **Jobs (M19):** `awaiting_session` exempt from `JOB_TTL`; Cloud Tasks is the only retry layer.
12. **App folder (H7):** `If-Match` conflict and retry; primary mailbox move on disconnect.
13. **Deletion order (M7)** and the UN-06 (5 business days) and ST-02 (14 days) timing cases (L10).
14. **New S5 test IDs:** SES-2, SES-3, WA-1, CFG-1, EXP-3, and JOB-1 on the `jobs` row.
15. **CSV filename** is server-fixed; CSV matches JSON.
16. **CSRF** defence is now named, so 7.2 can drop "the defence S6 picks". L5: the cookie deviation is decided, not pending.
17. **Test naming:** the register uses `asvs_<row>_*` (for example `asvs_v6_3_3_*`); S10's `asvs_v8_<row>_` looks like a typo.
18. **v2:** page handler tests, the SSRF suite for Chromium, the Outlook.com sandbox mailbox (T1) and Microsoft corpus rows move to v2.
