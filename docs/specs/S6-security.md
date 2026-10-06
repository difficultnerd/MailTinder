# S6: Security Specification

Status: DRAFT for James's review. 3 October 2026.
Target: OWASP ASVS 5.0 Level 2 (mandatory). Requirement-level mapping: `docs/security/asvs-l2-register.md`.
Depends on: `S3`, `S4`, `S5`, `research/data-handling-and-in-account-ai.md`.
Sign-in: Google OAuth only for the trial; the passkey lock is dropped and moves to v2 pre-CASA hardening (James, 3 October 2026).
Updated 3 October 2026 (spec audit): Google OAuth sign-in with invite token and one session per user (H3, H4, M2 as revised), one DKIM rule (H5), step-up by fresh Google sign-in (M1 as revised), one per-user `data_key` (H1 as revised), sealed tokens (M3), CSRF (M13), KMS holders (M14), security events (L6). James's answers, same date: passkey lock dropped for the trial (v2); Microsoft moves to v2 (H8); mailto stays in v1 and the page handler moves to v2 (M18); Jev stays open to consenting users with a retention disclosure (M17); SSRF controls apply in v1 to the `unsub` one-click POST.

## 1. Assets

| Asset | Why it matters |
| --- | --- |
| Mailbox access (refresh and access tokens) | Full read and modify of a person's email; the highest-value asset |
| Users' mail content in transit | Personal data; never at rest, but present in memory |
| Users' habits (sort rules, History) | Reveals relationships and interests |
| Unsubscribe and send capability | Could be abused to send mail as the user or unsubscribe them from things they want |
| Admin capability | Controls who gets in |
| Encryption keys | Unlock everything above |

## 2. Trust boundaries

1. Browser to Firebase Hosting and `api` (internet).
2. `api` to Google APIs (internet, authenticated); Microsoft APIs in v2.
3. `api` to Firestore, KMS, Secret Manager, Cloud Tasks (Google internal, IAM).
4. Cloud Tasks to `unsub` (OIDC token).
5. `unsub` to one-click unsubscribe targets taken from mail headers (untrusted internet hosts). v2: `unsub` to `pagehandler` (OIDC token), and `pagehandler` to arbitrary internet hosts (untrusted).
6. Mail content entering from senders (untrusted, possibly hostile).

## 3. Threat model (STRIDE, top threats)

| ID | Threat | Category | Control | ASVS |
| --- | --- | --- | --- | --- |
| T1 | Database leak exposes tokens | Information disclosure | Envelope encryption (section 5): refresh tokens under the per-user `data_key`, wrapped by Cloud KMS; KMS encrypt and decrypt held by three service accounts only (`api`, `unsub`, `worker`); a database copy alone yields nothing | V11, V14 |
| T2 | Hostile email content attacks the app (XSS, injection via subject or body) | Tampering | Plain-text rendering only; server-side HTML stripping; strict CSP with Trusted Types | V1, V3 |
| T3 | An unsubscribe target from a mail header performs SSRF to internal or metadata endpoints | Elevation of privilege | v1 (`unsub` one-click POST): https only; host resolved and private, loopback, link-local, CGNAT and metadata ranges refused (including 169.254.169.254 and `metadata.google.internal`); resolved IP pinned for the connection against DNS rebinding; no redirects followed (section 6). v2 scope (page handler): separate no-role service account; the same checks on every request and redirect; Direct VPC egress firewall | V1, V12, V13, V15 |
| T4 | Unsubscribe page or link content manipulates an LLM judge | Tampering | Page content is data only; judge output constrained to an enum; no tool access | V15 |
| T5 | Cross-tenant access (user A acts on user B's mailbox or job) | Elevation of privilege | Ownership check on every request and in every worker; deny by default; isolation tests | V8 |
| T6 | Phishing a user's sign-in | Spoofing | Google OAuth with PKCE, state and nonce; Google 2-Step Verification (`amr` checked where present); step-up by fresh Google sign-in. Phishing-resistant passkeys are v2 | V6, V10 |
| T7 | Session theft or fixation | Spoofing | `__session` cookie (name required by Firebase Hosting) with HttpOnly, Secure, SameSite=Lax, Path=/ and no Domain; rotation at sign-in and step-up; one session per user; idle and absolute timeouts | V3, V7 |
| T8 | CSRF triggers trash or unsubscribe | Tampering | Synchroniser token (`X-CSRF-Token`, bound to the session) plus `Origin` check on every `POST`, `PATCH` and `DELETE`, on top of SameSite=Lax; no CORS headers sent (S7 3.2) | V3, V4 |
| T9 | Abuse of send capability (mailto unsubscribe used to spam) | Elevation of privilege | Send only to the exact address in a validated `mailto:` List-Unsubscribe header from that same message, and only when a passing DKIM signature covers that header (section 6); per-user daily send cap | V2 |
| T10 | Uninvited sign-up, invite brute force or invite spoofing | Spoofing | Single-use 256-bit invite token from the invite email plus a match on the provider-verified email (v2 Microsoft: `xms_edov` true); rate limits; generic errors | V2, V6 |
| T11 | Malicious one-click target (unsubscribe link used as a tracking or attack beacon) | Information disclosure | POST only when DKIM covers both headers; no cookies or credentials; fixed body; HTTPS only | V12 |
| T12 | Sensitive data leaks into logs or error reports | Information disclosure | Allowlisted log fields; redacting types; Semgrep privacy rules; log-scanning test | V14, V16 |
| T13 | Destructive automation runs wild (mass trash) | Denial of service to the user | Trash only, never delete; History for every action; per-user action rate cap; undo | V2 |
| T14 | Supply chain compromise | Tampering | cargo-audit, cargo-deny, Dependabot with cooldown, pinned Actions, Artifact Registry scanning | V15 |
| T15 | Misconfigured cloud permissions | Elevation of privilege | Terraform only, reviewed plans; least-privilege service accounts; Security Command Center | V13 |
| T16 | Spoofed sender triggers rule actions on wanted mail | Spoofing | Sender keys only from DKIM-aligned or provider-authenticated messages; suspect mail never creates rules | V2 |
| T17 | Marketing email crafted to make a model mark it personal or transactional | Tampering | Header guard (S4 5.2); models cannot create unsubscribe or spam actions; badge stays on header rules during the bake-off; fixed question text | V15 |
| T18 | Mail content retained or exposed by TypeSafe or Vertex AI | Information disclosure | Allowlisted, redacted input (S4 5.5); consent whose text discloses "TypeSafe has not yet committed to how long it keeps this data and has no data processing agreement or security attestation in place" (James, 3 October 2026: Jev open to all consenting users before a DPA); pilot only; kill switches; Vertex zero-retention settings; Jev vendor terms gate before Google verification (S4 5.8) | V14 |
| T19 | Model outage or slow responses stall the Feed | Denial of service | Calls in parallel off the card's critical path; 2-second timeout; concurrency cap | V15 |
| T20 | Jev API key leaked | Information disclosure | Secret Manager, single accessor, never logged, quarterly rotation | V13 |
| T21 | A compromised Google account signs in to Mail Tinder | Spoofing | Out of the app's control for the trial (Google account security and 2-Step Verification); invite-only; step-up for every account, mailbox or admin change; sign-in events logged. v2 passkey lock adds a second, app-held factor | V6 |
| T22 | A sealed token replayed as another type, for another user or after expiry | Tampering | One sealing scheme with type, user ID, session record ID and expiry in the associated data (section 5) | V9 |

## 4. Authentication and session design

Authentication pathways (ASVS V6.1.3, V6.3.4): one pathway, Google OAuth, for first sign-in, sign-in, mailbox linking and step-up, so authentication strength is the same everywhere. No passkeys, WebAuthn or PRF in v1; the passkey lock moves to the v2 pre-CASA hardening backlog (James, 3 October 2026). There is no recovery flow in the app; Google account recovery applies.

- **Sign-in:** Google OAuth with any linked Gmail mailbox, matched by Google `sub`. Authorisation code flow with PKCE, `state` and `nonce`, exact redirect URIs, confidential web client. ID tokens validated (signature, issuer, audience, expiry, nonce).
- **Multi-factor (ASVS V6.3.3):** relies on Google 2-Step Verification. Where Google returns an `amr` claim, it is checked and recorded with the sign-in event. Otherwise this is an accepted trial deviation (register V6.3.3), closed by the v2 passkey lock.
- **First sign-in (enrolment):** the invite email carries a single-use invite token: 256-bit CSPRNG value, stored only as its SHA-256 hash, expires 7 days after sending `[TUNABLE]`, voided on use. Redemption needs the token, `email_verified` true and a match between the invited address and the Google email. Re-sending an invite issues a new token and voids the old one. v2 Microsoft rule: accept the email only when the `xms_edov` claim is true; otherwise redemption fails with a generic error.
- **Mailbox linking:** the same OAuth flow. Mailboxes keyed by provider plus provider subject, never by email: Google `sub`; v2 Microsoft `tid` plus `oid` (ASVS V6.8.1).
- **Scopes (Gmail):** `gmail.modify`, `gmail.send`, `drive.appdata`, `openid`, `email`. Microsoft (v2): `Mail.ReadWrite`, `Mail.Send`, `Files.ReadWrite.AppFolder`, `offline_access`, `openid`, `email`.
- **Sessions:** one session per user; a new sign-in ends the old one (ASVS V7.1.2). Opaque 256-bit random ID; stored hashed in Firestore; idle timeout 15 minutes, absolute 12 hours (decided); rotated at sign-in and at step-up; ended on sign-out and account deletion. Each session record also has a stable record ID that rotation does not change (used by sealed tokens, section 5). Settings has "Sign out" only. An admin can end a user's session, which deletes the one record (ASVS V7.4.5). With one session per user, V7.4.3 and V7.5.2 are met without a session list.
- **Re-authentication (step-up, ASVS V7.5.1):** a fresh Google sign-in within the last 5 minutes before any account, mailbox or admin change: delete account, link or disconnect a mailbox, and every admin write (including kill switches and snapshots). The request uses `prompt=login` and `max_age=300`, and the ID token's `auth_time` must be within 5 minutes. The step-up sign-in must be with one of the user's linked Google accounts (matched by `sub`); any other account is refused (`step_up_wrong_account`) and nothing changes.
- **Admin:** a role flag on the user; admin routes separate, rate limited and logged; every admin write needs step-up.

## 5. Cryptographic inventory

Each user has one key, `data_key`, usable by the server without the user, so queued unsubscribe jobs can run after the user leaves. The passkey-locked key (`token_key`, PRF and session custody) is dropped for the trial and moves to v2 (James, 3 October 2026: "if they breach google KMS the world has bigger problems").

| Key or secret | Algorithm | Protects (and may not protect) | Held by | Rotation and end of life |
| --- | --- | --- | --- | --- |
| KMS key encryption key (`data-key-kek`) | AES-256 (Cloud KMS software key) | Wraps every `data_key`; nothing else | Cloud KMS; encrypt and decrypt granted to the `api`, `unsub` and `worker` service accounts only | Yearly, automatic; old versions kept for unwrap |
| System KMS key (`system-fields`) | AES-256 (Cloud KMS software key), direct encryption with associated data = record ID plus field name | Data that exists before a user does: invite and invite request email addresses, `pre_auth` sign-in fields. Never user data | Cloud KMS; encrypt and decrypt granted to `api` only; James's account for the admin tool (T-507) | Yearly, automatic; old versions kept for decrypt |
| `data_key` (per user) | AES-256-GCM; associated data = user ID plus mailbox ID plus field name for refresh tokens, user ID plus field name or token type otherwise | Refresh tokens, the app folder file, all sealed tokens (below) and encrypted Firestore fields. Never mail content (none is stored) | Firestore, wrapped under KMS; usable by `api`, `unsub` and `worker` | On demand (re-encrypt); destroyed on deletion |
| Email lookup HMAC key | HMAC-SHA-256 | Keyed hashes for invite and email lookup | Secret Manager, `api` | Yearly |
| Log pseudonymisation HMAC key | HMAC-SHA-256 | Pseudonymous user IDs in logs (S5 C1 fields) | Secret Manager | Yearly |
| Session IDs | 256-bit CSPRNG, stored as SHA-256 | Session reference | Cookie (raw); Firestore (hash only) | Per session; rotated at sign-in and step-up |
| Invite tokens | 256-bit CSPRNG, stored as SHA-256 | Invite redemption | Invite email (raw); Firestore (hash only) | Single use; 7 days `[TUNABLE]` |
| OAuth client secrets | Provider-issued | Token requests | Secret Manager | On provider rotation |
| Jev API key | Vendor-issued | Jev calls | Secret Manager, `api` only | Quarterly |

**Access tokens.** No access token is stored on a job. A queued unsubscribe job mints an access token from the stored refresh token when it runs; if the refresh token is revoked or invalid, the job ends `failed` with a Needs Attention item "Sign in again".

**Sealed tokens (one scheme).** Every server-issued opaque token (feed cursors, undo tokens, classification tokens, prompt refs) is AES-256-GCM under the user's `data_key`, with a random 96-bit nonce and associated data = token type, user ID, session record ID and expiry. The token carries its type and expiry in clear (authenticated as associated data). The receiving route rebuilds the associated data from the type it expects, not the type the token claims, so a token of one type is rejected as another (ASVS V9.2.2); it rejects expired tokens (V9.2.1). The session record ID is stable across session rotation, so linking a mailbox or a step-up keeps the undo stack. No separate token key exists.

**Deletion.** Account deletion destroys the wrapped `data_key` (crypto-shredding), after the steps that still need it (S2 AU-06). The trial runs with no Firestore backups, point-in-time recovery or scheduled exports, so no older copy of the wrapped key survives (James, 4 October 2026).

## 6. Unsubscribe execution rules

- **One DKIM rule for every method** (one-click, mailto, the v1 Needs Attention page link, and the v2 page handler): an unsubscribe job is created only when a passing DKIM signature covers `List-Unsubscribe`, and also `List-Unsubscribe-Post` for one-click. DMARC pass is not required. Otherwise the message class is `bulk_no_header`: trash plus reject rule, no unsubscribe job.
- One-click: POST only under the DKIM rule. HTTPS only. SSRF checks before connecting: resolve the host, refuse private, loopback, link-local, CGNAT and metadata ranges (including 169.254.169.254 and `metadata.google.internal`), and pin the resolved IP for the connection so DNS rebinding cannot change it. No cookies, no credentials, fixed body, 10-second timeout. No redirects are followed: a 3xx response raises a Needs Attention item and nothing is retried.
- Mailto: only under the DKIM rule. Send only to the address in that header, with its subject and body parameters only; reject any other header parameters (`cc`, `bcc`).
- Https-only List-Unsubscribe without one-click (v1): no automated action; a Needs Attention item with "Open unsubscribe page" for the user, using the link from the DKIM-covered header only.
- Page handler (v2 scope, rules kept): only for an https link in the List-Unsubscribe header without one-click, with the one-click SSRF checks on every request and redirect plus its own isolation (T3). Never for links found in the body. Never for messages without a List-Unsubscribe header.
- Never act on mail classed `suspect`.

## 7. Security logging

Events logged (pseudonymous, C1 fields only): sign-in success and failure (with `amr` where present), step-up success and failure, session termination (sign-out, replaced by a new sign-in, or ended by an admin), mailbox link and unlink, invite created, revoked, used, request approved or declined, consent changes (experiments opt-in and opt-out), kill switch changes, bake-off snapshot save and delete, admin actions, authorisation failures, CSRF failures, rate-limit hits, job outcomes, account deletion. Log bucket locked, 90-day retention, no delete permission for application identities.

A log line carries at most one pseudonymous user ID (S5 logs). An action with two principals is therefore logged as two correlated lines, never one: an admin ending a user's session emits a session-termination line (`session_end`/`admin_ended`) named for the target user and an admin-action line (`admin_action`) named for the acting admin. The two lines share the request ID, which is the documented join key that attributes the termination to the admin. Neither line carries an address.

## 8. Security verification in CI

| Check | Tooling | Status |
| --- | --- | --- |
| Secrets | Gitleaks | In template |
| Rust advisories and licences | cargo-audit, cargo-deny | In template |
| Static analysis | Clippy (deny warnings), Semgrep | In template |
| Privacy rules | `optional/privacy` layer, extended with S5 field names | To install |
| ASVS register coverage | Script checks each register row marked Verified cites a passing test | To build |
| Dynamic scan of staging | `optional/zap` layer | To install before trial |
| Container scanning | Artifact Registry scanning | Cloud side, not CI core |

## 9. Decisions for James

1. **Session timeouts:** decided, 15 minutes idle and 12 hours absolute (James, 3 October 2026).
2. **Log retention:** decided, 90 days (James, 3 October 2026).
3. **Cookie prefix deviation:** decided. `__session` without the `__Host-` prefix accepted for the trial only (James, 3 October 2026); replaced by `__Host-session` when the pre-production load balancer routes `/api` directly to Cloud Run.
4. **Sign-in:** decided, Google OAuth only. The passkey lock is dropped for the trial and moves to v2 pre-CASA hardening (James, 3 October 2026). Accepted trial deviation: ASVS V6.3.3 relies on Google 2-Step Verification (`amr` checked where present), closed by the v2 passkey lock.
5. **Threat model sign-off** once reviewed.
6. **Pre-user data:** decided, a second KMS key `system-fields` (James, 4 October 2026).
7. **Backups:** decided, none for the trial (James, 4 October 2026).
8. **First admin:** decided, set by the `mt-admin` tool outside the API (James, 4 October 2026).
