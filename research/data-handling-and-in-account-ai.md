# Mail Tinder: Minimal Data Handling and In-Account AI

Last updated: 3 October 2026 (revised the same day for multiple mailboxes per user)
Status: research recommendation. Decided so far: passkey lock for mailbox tokens (James, 3 October 2026). Everything else is pending confirmation. Research only; no code.

## Recommendation

1. **Hold no mail content at rest.** The backend reads mail from the provider on demand, streams card data to the browser and discards it. No message bodies, subjects, snippets, headers or embeddings go into our database, logs, caches or backups.
2. **Store refresh tokens, locked to the user's passkey.** Users connect several mailboxes at once (James has three Gmail accounts and several Microsoft 365 accounts), so a provider sign-in per mailbox per session is not workable. Keep an encrypted refresh token per mailbox, encrypted under a per-user key that only the user's passkey can unlock (WebAuthn PRF). One passkey tap per session unlocks every mailbox. A stolen database, even with our cloud KMS, yields no usable token.
3. **Keep the user's own state in the user's own account.** Sort rules, filing categories and the user-facing History live in a hidden app folder in the user's Google Drive (`drive.appdata`, non-sensitive) or OneDrive (`Files.ReadWrite.AppFolder`), plus labels or categories on the messages themselves. Our server keeps only what it needs to run jobs and stay secure.
4. **Use Google's own classifiers first, and Google's on-device model second.** Gmail's `CATEGORY_*` labels and spam handling are free and run inside the user's account. For anything smarter, run Gemini Nano in the user's Chrome through the Prompt API: Google's model, on the user's device, no content leaves it. No server-side LLM in v1.
5. **Accept that CASA applies regardless.** Any design where our server can touch Gmail data triggers the security assessment. Minimal state does not lower the tier Google assigns, but it shrinks what the assessor has to examine and what a breach can expose.
6. **Install the template's `optional/privacy` layer on day one** and extend it (see Logging).

This changes two earlier recommendations: `docs/plan-open-questions.md` section C ("store message IDs, headers, sender, subject and a short snippet") and `research/cheap-email-classification.md` layer 2 (vectors stored in Postgres with `pgvector`). See "Suggested changes" at the end.

## Part 1: what state is unavoidable

Each feature James has decided on, and the least state it needs.

| Feature | State needed | Where it lives | Retention |
| --- | --- | --- | --- |
| Login and invite check | Internal user ID, passkey public key, provider subject ID per linked mailbox, allowlist entry | Our database | Life of account |
| Mailbox access | One refresh token per linked mailbox, encrypted under the passkey-locked user key | Our database | Until the mailbox is removed or the token expires |
| Feed | Nothing. Page cursor (provider page token, or newest and oldest message ID seen) | Browser session; cursor in user's app folder so the Feed resumes | Session; cursor overwritten |
| Swipe and undo | The last action: message ID, action, previous label set | Browser memory; server job record if an unsubscribe is queued | Until next swipe or job completion |
| Queued unsubscribe (few-minute delay) | Message ID, unsubscribe target (URL or mailto), sender address, encrypted access token, due time | Our database, encrypted | Deleted on completion or cancel; hard TTL 1 hour |
| Needs Attention | Sender display name and address, unsubscribe URL, failure reason | Our database, encrypted (needed for notifications) | Deleted on resolve; hard TTL 30 days |
| Sort rules (per sender or type) | Sender address or domain, action, category | User's app folder | User controls; deleted with account |
| Filing categories and keep-learning | Category names, sender to category map, keep counts per sender | User's app folder; categories also exist as Gmail labels or Outlook categories | User controls |
| History (user-facing) | What the app did, when, to which sender | User's app folder (append log) plus labels on messages | User controls; trim at, say, 12 months |
| Security audit log | Pseudonymous user ID, action type, outcome, time, request ID. No addresses, subjects or URLs | Our log store | 90 days (assessor will ask for a figure) |
| Unsubscribe delivery check | Sender address and date unsubscribed | User's app folder; check runs at next sign-in | Cleared after check |
| Push notifications | Push subscription endpoint | Our database | Until revoked |

### Why the app folder

- Google classes `drive.appdata` as non-sensitive, so it adds no verification burden ([Drive scopes](https://developers.google.com/workspace/drive/api/guides/api-specific-auth)). The folder is hidden from the user's Drive UI and only our app can read it.
- Microsoft's app folder works for both personal and work OneDrive with delegated access ([Graph app folder](https://learn.microsoft.com/en-us/graph/onedrive-sharepoint-appfolder)).
- If our database leaks, it holds no picture of anyone's mail habits. If the user leaves, their rules leave with them.
- Costs: one extra scope on the consent screen; the user can delete the file (Microsoft warns of this), so the app must tolerate a missing or corrupt file and rebuild from labels; reads and writes add latency, so cache the file in the browser for the session.
- Encrypt the app folder file with a key held by our server (AES-256-GCM, per-user key). That keeps it opaque if the user's Drive is shared or compromised, and means a stolen copy is useless without our key. Trade-off: the user cannot read it directly. Acceptable, because the History screen is the user's view.

### Multiple mailboxes per user

James's requirement (3 October 2026): one user, many mailboxes, several per provider, all in one Feed.

- **Identity.** An internal user record owns many linked mailboxes. Any linked mailbox can be added or removed; the user signs in to Mail Tinder itself with a passkey (below), so no single mailbox is the identity. The invite check runs on the first mailbox connected.
- **Home app folder.** User-level state (sort rules that apply across mailboxes, History, Feed cursors) lives in one "home" app folder the user picks, defaulting to the first personal Gmail connected. Avoid a work Microsoft 365 account as home: leaving the employer takes the state with it, and tenant admins may block app folder consent. Mailbox-specific state (filing categories as labels or folders) stays in that mailbox.
- **Work Microsoft 365 tenants.** Each tenant's admin consent policy may block `Mail.ReadWrite`, and Conditional Access can force re-authentication on its own schedule. Expect some work mailboxes to need a fresh sign-in more often than personal ones; show that per mailbox in Settings rather than failing the whole Feed.
- **Google limits.** Google caps the number of live refresh tokens per Google account per OAuth client (older tokens are silently invalidated past the cap), so one user connecting the same Gmail account repeatedly must replace, never add, its token.

### Refresh tokens and the passkey lock

The first draft recommended no refresh tokens and a provider sign-in every session. With several mailboxes that means several OAuth round trips every time the app opens, which kills the "fun to use" goal. Refresh tokens are needed. The question becomes who can use them.

A refresh token for `gmail.modify` or `Mail.ReadWrite` is a long-lived key to a mailbox, and with many mailboxes per user our database becomes a high-value target. Three ways to hold them:

| Option | Who can decrypt the tokens | Background work when the user is away | Breach exposure | Friction |
| --- | --- | --- | --- | --- |
| A. Server key only (KMS envelope encryption) | Our backend, any time | Yes: polling, delivery checks, push alerts | Database plus KMS access exposes every mailbox | None after first connect |
| B. **Passkey lock (recommended)** | Our backend, only after the user unlocks with their passkey in this session | Only during the session plus up to about an hour of queued jobs | Database plus KMS yields nothing usable; attacker also needs each user's passkey | One passkey tap (Face ID, Touch ID, Windows Hello) per session |
| C. No refresh tokens | Nobody | None | Nothing stored | One provider sign-in per mailbox per session |

How option B works:

1. The user registers a passkey with Mail Tinder. The passkey is also the Mail Tinder sign-in, which suits ASVS V6 (phishing-resistant authentication).
2. At sign-in, the browser asks the passkey for a PRF output (a secret derived inside the authenticator, unique to our site and a fixed salt). That secret never exists on our server at rest.
3. The backend derives a key-encryption key from the PRF output plus a server-held KMS key (both needed), and uses it to unwrap the user's data key, which in turn decrypts each mailbox's refresh token. The unwrapped data key lives in memory for the session only.
4. The backend exchanges refresh tokens for access tokens (about an hour each), stores rotated refresh tokens back under the same data key, then drops the data key at sign-out or idle timeout.
5. Queued unsubscribes and agent jobs carry their own encrypted access token with a one-hour hard TTL, so the few-minute delay and the fallback agent still finish after the user closes the app.

What option B costs:

- **No background work while the user is away.** New-mail polling and the 14-day unsubscribe delivery check run at next sign-in. That suits a pull-based triage app. If v2 needs true background work (Gmail filters, scheduled bulk jobs, proactive alerts), let users opt individual mailboxes into option A.
- **PRF support.** Supported by Google Password Manager on Android and Chrome, iCloud Keychain on Safari 18 and later (fixed bugs in iOS 18.4 and later), and Windows Hello since the February 2026 Windows 11 update; third-party password managers are inconsistent (Bitwarden fails on macOS) ([Corbado PRF guide](https://www.corbado.com/blog/passkeys-prf-webauthn)). Fallback for a passkey without PRF: that user's tokens use option A, disclosed in Settings. Measure the trial group's mix before making PRF mandatory.
- **Lost passkey.** The tokens become undecryptable, so the user reconnects each mailbox. No data is lost, because state lives in their own app folders. Encourage a second passkey (synced passkeys usually cover this).

Token lifetimes that shape the design:

- Google issues a refresh token only with `access_type=offline` ([Google OAuth for web servers](https://developers.google.com/identity/protocols/oauth2/web-server)). In Testing mode Google expires refresh tokens after 7 days, so trial users reconnect Gmail weekly regardless of option.
- Microsoft refresh tokens last 90 days for a confidential client (our backend) and rotate on every use; the old one must be deleted. A redirect URI registered as `spa` gets only 24 hours, another reason for the backend-for-frontend pattern. Password resets and admin revocation kill them ([Microsoft refresh tokens](https://learn.microsoft.com/en-us/entra/identity-platform/refresh-tokens)).
- Revoke at the provider on disconnect: `https://oauth2.googleapis.com/revoke` for Google; for Microsoft, delete our copy and tell the user they can remove the app's access from their Microsoft account page (I found no per-token revocation endpoint for third-party apps; confirm in the spike).

**Decided (James, 3 October 2026):** passkey lock (option B) for v1. Option A stays available only as an explicit per-mailbox opt-in if a v2 feature needs background access, and as the disclosed fallback for passkeys without PRF.

### Encryption

- **In transit:** TLS 1.2 minimum, TLS 1.3 preferred, HSTS, no plaintext fallback. Server to provider and to unsubscribe targets over HTTPS only; refuse plain `http` unsubscribe links or route them to Needs Attention.
- **At rest, database:** managed Postgres encryption at rest (provider default) plus application-level envelope encryption for every column that holds a token, address or URL. Per-user data encryption key, wrapped by a key in a cloud KMS. AES-256-GCM with the user ID and column name as associated data, so a ciphertext copied to another row fails to decrypt.
- **Crypto-shredding:** deleting the user's wrapped key makes every backup copy of their encrypted fields unreadable. This answers "delete my data" for backups we cannot rewrite.
- **Keys:** KMS key never leaves KMS; token data keys are additionally bound to the user's passkey PRF output (option B above); the app holds unwrapped data keys in memory only, for the session; rotate the KMS key yearly. KMS audit logs record every unwrap, which gives the assessor evidence of who touched token keys.
- **Browser:** no tokens in `localStorage` or `sessionStorage`. Session cookie is `HttpOnly`, `Secure`, `SameSite=Lax` or `Strict`, `__Host-` prefixed. Card data lives in memory only; nothing written to IndexedDB.

### Token handling pattern

Use a backend-for-frontend: the Rust backend runs the OAuth code flow with PKCE, holds the provider access token, and gives the browser only a session cookie. ASVS 5.0 chapter V10 (OAuth and OIDC) expects tokens to reach only the components that need them, which for a browser app means the backend. This rules out the browser calling the Gmail API directly with its own token, even though that would keep mail off our server.

### Logging and error reporting

- Allowlist, not denylist: log only named fields (request ID, pseudonymous user ID, route, status, latency, action type, outcome code).
- Never log: tokens, cookies, message IDs, addresses, display names, subjects, snippets, bodies, unsubscribe URLs (they carry per-recipient identifiers), page content from the unsubscribe agent.
- Rust: wrap sensitive values in a redacting type (for example the `secrecy` crate) so `Debug` and `Display` print a placeholder. Ban `Debug` derives on structs holding them.
- Error reports: strip request bodies and query strings; no third-party error service in v1 unless it is self-hosted or contractually scrubbed.
- The template's `optional/privacy` layer already bans `println!`, `eprintln!` and `dbg!` and flags logging of sensitive names with Semgrep. Install it, add the field names above to its rules, and add `privacy-checks` to branch protection.
- Note: that layer's checklist says "nothing is written to disk, cache or database". Our design does write a little (job queue, Needs Attention). Edit `PRIVACY.md` to state the real commitment: no mail content at rest, the listed exceptions, each with its TTL.

### Unsubscribe agent workers

- Each attempt runs in a fresh headless browser context with no profile, cookies or storage, destroyed afterwards.
- No access token goes to the worker; the worker returns an outcome and the backend applies any mailbox change.
- Screenshots or page captures for debugging stay off by default. If enabled for a Needs Attention item, they expire with it.

### Deletion

"Delete my account" in Settings: revoke tokens at Google (`https://oauth2.googleapis.com/revoke`) and Microsoft, delete database rows, destroy the user's wrapped key, delete the app folder file (we hold the scope to do so), optionally remove the app's labels. Log the deletion pseudonymously.

## Part 2: Google AI inside the user's own account

James asked whether Gmail classification can be limited to Google AI services running within the user's own account. Options, best first.

| Option | Runs where | Data leaves user's control? | Works for friends? | Verdict |
| --- | --- | --- | --- | --- |
| Gmail system labels (`CATEGORY_PROMOTIONS`, `UPDATES`, `SOCIAL`, `FORUMS`, `PERSONAL`, spam, importance) | Google, inside the user's account | No | Yes, free | **Use.** Already layer 1 of the classification doc. |
| Gemini Nano in Chrome (Prompt API) | User's device | No; Google says prompts never leave the browser | Desktop Chrome on capable hardware; not iOS, Safari or Firefox | **Use as the smart layer**, with a fallback. |
| Gemini in Gmail (Workspace or Google One AI plans) | Google, inside the user's account | No | Only users with a paid plan | **Not possible.** No developer API to invoke it on a user's behalf; none found in Google's documentation. |
| Apps Script in the user's account calling Gemini via Vertex AI | Google, user's own Cloud project | No (user's own project) | No: each user needs a Google Cloud project with billing enabled | **Reject for the trial.** Too much friction; Rust backend and Outlook would be bypassed. |
| Gemini API free tier | Google | Yes: Google may use content to improve products and humans may read it | Yes | **Reject.** Incompatible with mail. |
| Gemini on Vertex AI or paid Gemini API, in our project | Google Cloud, our tenancy | Yes, to Google as our processor; no training; ZDR possible | Yes | **Hold for v2 edge cases**, opt-in. |

### Gemini Nano through the Chrome Prompt API

- Stable for web pages from Chrome 148 (Google I/O 2026), with structured JSON output ([Chrome at I/O 2026](https://developer.chrome.com/blog/chrome-at-io26?hl=en)).
- Runs entirely on the device; Google states the prompt never leaves the browser process.
- Requirements (secondary source, verify in the spike): Windows 10 or 11, macOS 13 or later, Linux or ChromeOS; about 22 GB free disk and 16 GB RAM, or a GPU with 4 GB VRAM; the model download is 2.7 to 4 GB. Not on iOS, Safari or Firefox ([pasqualepillitteri.it summary](https://pasqualepillitteri.it/en/news/3145/gemini-nano-chrome-built-in-ai-client-side-en)).
- Flutter web can call it through JS interop.
- It works for Outlook mail too, since it runs on card data the browser already has.
- Fit: proposing a category name for a new type, judging marketing versus transactional on a card, explaining the bulk badge. It does not suit work that must run when the user is away, which the passkey-locked token design gives up anyway.
- Fallback when unavailable (phones, Safari, low-spec laptops): header rules and Gmail labels only, plus asking the user. Server-side local embeddings (classification doc layer 2) remain an option, computed in memory and never stored; measure in the spike whether they add enough over sender history to justify the memory.

### Policy check

- Google's Workspace API policy bans transferring Gmail data to generalised AI models and retaining it to train non-personalised models (see classification research). On-device Gemini Nano with no training is the cleanest possible fit.
- Gemini API unpaid tier terms: Google may use submitted content to improve products, and human reviewers may read it ([Gemini API terms](https://ai.google.dev/gemini-api/terms)). That also breaches Google's own user data policy on human reading of mail. Never use it.
- Vertex AI: Google does not train on customer data without permission; Gemini caches prompts in memory for 24 hours by default (can be disabled per project), and abuse-monitoring logging needs an exception request for zero retention ([Vertex AI data governance](https://docs.cloud.google.com/vertex-ai/generative-ai/docs/data-governance), [Vertex ZDR](https://docs.cloud.google.com/vertex-ai/generative-ai/docs/vertex-ai-zero-data-retention)).

### Does staying inside Google avoid CASA?

No, not for this product.

- Google: "Every app that requests access to Google users' restricted data and has the ability to access data from or through a third-party server must go through a security assessment" ([restricted scope verification](https://developers.google.com/identity/protocols/oauth2/production-readiness/restricted-scope-verification)). The Gmail scopes page says the same for storing or transmitting restricted data on servers ([Gmail scopes](https://developers.google.com/workspace/gmail/api/auth/scopes)).
- Our backend holds the access token and calls the Gmail API, and RFC 8058 one-click unsubscribe must come from a server (browsers cannot make that POST cross-origin), so the server path is unavoidable.
- A Google developer advocate has said Apps Script projects that never send Gmail data outside Google (no `script.external_request`) count as local clients and can skip the assessment ([Apps Script community thread](https://groups.google.com/g/google-apps-script-community/c/LODyY26RhnA)). A developer asked Google in 2026 whether on-device processing escapes CASA and got no answer ([Google developer forum](https://discuss.google.dev/t/gmail-restricted-scope-is-the-casa-assessment-required-when-mail-is-processed-on-device-and-only-a-derived-subscription-list-reaches-our-server/398372)). Treat any exemption as unavailable to us.
- `gmail.metadata` (headers only) is still restricted and cannot trash, so it does not help either.

The trial stays exempt through Testing mode (up to 100 named test users) or the personal use exemption.

## Part 3: OWASP ASVS Level 2 mapping

ASVS 5.0 (released May 2025) is the target. CASA is built on ASVS ([deepstrike CASA summary](https://deepstrike.io/blog/google-casa-security-assessment-2025)); as far as I could find it still maps to ASVS 4.0, so building to 5.0 L2 covers it with a translation table at assessment time. Verify the CASA requirement set against the ADA dashboard before the first assessment.

| ASVS 5.0 chapter | What it means for Mail Tinder | Design choice above |
| --- | --- | --- |
| V1 Encoding and Sanitization | Mail is hostile input: render subjects, names and snippets as text only; HTML bodies (if ever shown) in a sandboxed iframe with no scripts | Card shows text only in v1 |
| V2 Validation and Business Logic | Undo window, rate limits on unsubscribes and mailto sends, invite check, anti-automation | Job TTL, per-user rate limits |
| V3 Web Frontend Security | Strict CSP, Trusted Types, no inline script, frame-ancestors none, cookie flags | Flutter web build must be checked for CSP compatibility in the spike |
| V4 API and Web Service | Authenticated JSON API, CSRF protection on state-changing calls, strict CORS | BFF with session cookie |
| V5 File Handling | Never store or open attachments | Attachments out of scope |
| V6 Authentication | Passkey sign-in to Mail Tinder (phishing-resistant); mailbox OAuth verifies ID token signature, issuer, audience, nonce. No passwords | Passkey plus linked mailboxes |
| V7 Session Management | Server-side sessions, rotation on sign-in, idle and absolute timeouts, sign-out revokes | Idle timeout drops the unwrapped token key |
| V8 Authorization | Every request checks the mailbox belongs to the session user; job workers re-check ownership | Multi-tenant isolation tests |
| V9 Self-contained Tokens | Validate provider ID tokens; do not issue our own JWTs to the browser | Opaque session cookie |
| V10 OAuth and OIDC | Code flow with PKCE, `state` and `nonce`, exact redirect URIs, minimal scopes, tokens held only by the backend | BFF; refresh tokens rotated and passkey-locked |
| V11 Cryptography | Approved algorithms, KMS-held keys, no home-grown crypto, key rotation | Envelope encryption, AES-256-GCM |
| V12 Secure Communication | TLS 1.2 or higher everywhere, HSTS, certificate validation on outbound calls | HTTPS-only unsubscribe targets |
| V13 Configuration | Secrets from a secret manager, never in the repo (Gitleaks already runs), hardened headers, no debug endpoints | Template toolchain |
| V14 Data Protection | Data classification, minimisation, retention limits, no sensitive data in URLs, logs or caches; `Cache-Control: no-store` on mail responses | Parts 1 and 2 |
| V15 Secure Coding and Architecture | Dependency hygiene (cargo-audit, cargo-deny, Dependabot already in template), threat model, isolation of the unsubscribe agent | Ephemeral browser workers |
| V16 Security Logging and Error Handling | Log security events with enough to investigate, nothing sensitive; protect log integrity; generic errors to users | Allowlist logging, pseudonymous IDs |
| V17 WebRTC | Not applicable | |

What makes this "spec-led": each applicable requirement becomes a row in a requirements register with the control, the code location and the verification method (unit test, integration test, Semgrep rule, CI check or manual review). The product plan thread should decide where that register lives; a reasonable default is `docs/security/asvs-l2-register.md` in the repo, filled before the first feature PR.

Requirements that need a human decision rather than a design choice: log retention period (90 days proposed), session idle and absolute timeouts (15 minutes idle, 12 hours absolute proposed), and the threat model sign-off.

## Part 4: Outlook equivalents

- **Native signals:** `inferenceClassification` (Focused or Other) and junk folder. Coarser than Gmail categories.
- **In-account AI:** Microsoft 365 Copilot exposes no API a third party can call on a consumer mailbox; none found. Gemini Nano in Chrome covers Outlook mail equally, since it runs on card data.
- **App folder:** `Files.ReadWrite.AppFolder` works for personal and work OneDrive.
- **Tokens:** request `offline_access`, register the redirect URI as a web (confidential) client to get 90-day rotating refresh tokens, and store each rotated token under the passkey lock.
- **Assessment:** no CASA equivalent. Microsoft publisher verification removes the "unverified" prompt; Microsoft 365 App Compliance certification is optional and not needed for the trial. The same ASVS L2 build covers it.

## Effect on Google's CASA tier

- Google assigns the tier from data sensitivity, user count and its own risk signals; the developer does not choose ([deepstrike](https://deepstrike.io/blog/google-casa-security-assessment-2025)). `gmail.modify` will likely draw the top tier (lab-tested, about USD 4,500 a year), matching James's planning assumption.
- Minimal state does not change the tier but cuts assessor questions about storage, encryption at rest, backups and retention to a short list, and leaves a breach with almost nothing to expose.
- Building to ASVS 5.0 L2 from the start means the assessment is evidence gathering, not a retrofit.

## Open items for the spike

1. Gemini Nano: availability on James's and a typical friend's hardware, latency per card, quality on bulk versus personal and category naming.
2. Flutter web under a strict CSP with Trusted Types.
3. Passkey PRF availability across the trial group's devices and password managers; re-authentication frequency on James's work Microsoft 365 tenants.
4. App folder read and write latency, and behaviour when the file is deleted.
5. Whether Gmail applies `CATEGORY_*` labels when inbox tabs are off (carried over from the classification doc).

## Suggested changes to other documents

For the product plan thread, which owns these files:

- `CONTEXT.md`, design principles: replace "Retain as little user data as possible; the data handling design is being researched" with "No mail content at rest on our servers. User state lives in the user's own account (app folder and labels). Server keeps only job, Needs Attention and security log records, each with a TTL." once James confirms.
- `CONTEXT.md`, providers and authentication: replace "The OAuth login doubles as the mail access grant" with "A user links many mailboxes, several per provider, used at once. Users sign in to Mail Tinder with a passkey; mailbox refresh tokens are stored encrypted and unlock only with that passkey" once James confirms.
- `plan-open-questions.md` B4 (identity versus connected mailboxes): superseded by the multiple mailboxes section here.
- `CONTEXT.md`, open questions: "How spam confidence is produced" now resolves to Gmail labels plus header rules, with Gemini Nano on-device as the smart layer.
- `plan-open-questions.md` section C, "What the server stores": supersede with Part 1 of this document.
- `research/cheap-email-classification.md` layer 2: vectors should not be stored in Postgres; compute in memory or drop in favour of sender history plus Gemini Nano. Layer 3 hosted LLM moves to v2, Vertex AI or paid Gemini API only, opt-in.

## Sources

- Google restricted scope verification: https://developers.google.com/identity/protocols/oauth2/production-readiness/restricted-scope-verification
- Gmail API scopes (restricted, sensitive classification; server storage triggers assessment): https://developers.google.com/workspace/gmail/api/auth/scopes
- Google API Services User Data Policy: https://developers.google.com/terms/api-services-user-data-policy
- Apps Script community thread on local client exemption: https://groups.google.com/g/google-apps-script-community/c/LODyY26RhnA
- Google developer forum question on on-device processing and CASA (unanswered): https://discuss.google.dev/t/gmail-restricted-scope-is-the-casa-assessment-required-when-mail-is-processed-on-device-and-only-a-derived-subscription-list-reaches-our-server/398372
- Google OAuth 2.0 for web server apps (offline access, revocation, DPoP): https://developers.google.com/identity/protocols/oauth2/web-server
- Microsoft identity platform refresh tokens (lifetimes, rotation, revocation): https://learn.microsoft.com/en-us/entra/identity-platform/refresh-tokens
- Passkeys and WebAuthn PRF support (secondary): https://www.corbado.com/blog/passkeys-prf-webauthn
- Yubico developer guide to PRF: https://developers.yubico.com/WebAuthn/Concepts/PRF_Extension/Developers_Guide_to_PRF.html
- Drive API scopes (`drive.appdata` non-sensitive): https://developers.google.com/workspace/drive/api/guides/api-specific-auth
- Drive app data folder: https://developers.google.com/workspace/drive/api/guides/appdata
- Microsoft Graph app folder: https://learn.microsoft.com/en-us/graph/onedrive-sharepoint-appfolder
- Apps Script Vertex AI advanced service (needs Cloud project with billing): https://developers.google.com/apps-script/advanced/vertex-ai
- Vertex AI data governance: https://docs.cloud.google.com/vertex-ai/generative-ai/docs/data-governance
- Vertex AI zero data retention: https://docs.cloud.google.com/vertex-ai/generative-ai/docs/vertex-ai-zero-data-retention
- Gemini API terms (unpaid versus paid data use): https://ai.google.dev/gemini-api/terms
- Chrome at Google I/O 2026 (Prompt API stable in Chrome 148): https://developer.chrome.com/blog/chrome-at-io26?hl=en
- Gemini Nano in Chrome requirements (secondary): https://pasqualepillitteri.it/en/news/3145/gemini-nano-chrome-built-in-ai-client-side-en
- CASA assurance levels and ASVS basis (secondary): https://deepstrike.io/blog/google-casa-security-assessment-2025
- App Defense Alliance CASA tier 2 overview: https://appdefensealliance.dev/casa/tier-2/tier2-overview
- OWASP ASVS 5.0: https://github.com/OWASP/ASVS/tree/master/5.0/en
- Repo template privacy layer: `optional/privacy/` in difficultnerd/MailTinder
