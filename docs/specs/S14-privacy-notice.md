# S14: Privacy Notice and Google Verification Pack

Status: DRAFT for James's review. 5 October 2026.
Depends on: `S5-data-inventory.md` (data location, retention, deletion), `S6-security.md` (KMS, deletion, security logging), `S4-architecture.md` (services, classifier bake-off), `docs/change-requests/CR-01-pluggable-classifier-jev-pilot.md` and `CR-01a-bakeoff-publishable-stats.md` (the bake-off consent).
Feeds: the Google OAuth verification submission, the app's Settings screens (S9), and the bake-off consent text (S2, S9).

This spec contains: (1) the full privacy notice for end users, including the Australian Privacy Principle 8 (APP 8) cross-border disclosure; (2) the Google Limited Use statement required for Google OAuth verification; and (3) the list of materials the Google OAuth verification pack needs.

## 1. Purpose

MailTinder is a swipe-to-triage email client. Its privacy design is set by S5: **no mail content at rest on our infrastructure**, and all server-side data held in the United States (`us-central1`). The privacy notice must say so, to meet Australian Privacy Principle 8 (cross-border disclosure) (S5 line 103). The bake-off consent (CR-01, CR-01a) depends on this spec: the consent text names the recipients and what is sent, and the Google Limited Use policy applies from the first user (S4 5.8, CR-01 2).

## 2. Privacy notice for end users

This is the full privacy notice text. It is written to be shown to end users (in Settings and at sign-in) and to be the basis of the Google OAuth verification submission. `[ASSUMES]` marks a placeholder James should confirm (company name, contact details, effective date).

### Mail Tinder Privacy Notice

*Effective date: [DATE] `[ASSUMES]`*

**What Mail Tinder is.** Mail Tinder is a swipe-to-triage email client. You swipe left to reject and unsubscribe from unwanted mail, right or up to keep it, and down to skip. It works with your Gmail account (Microsoft Outlook support is planned for a later version).

**What we do not store.** Mail Tinder does not store your mail content. Message bodies, subjects, snippets, headers and attachments exist only in server memory for the length of a request and in your browser's memory while a card is shown. They are never written to our database, our logs, or any other storage. We do not sell your data, and we do not use advertising trackers.

**What we do store.** We store only what the service needs, and everything has a purpose, a retention period and a deletion path:

- **Account and mailbox records** — your account, the mailboxes you link, and an encrypted copy of the access token that lets the app read and manage your mail. Tokens are encrypted with a key that only our servers can use, and are deleted when you disconnect a mailbox or delete your account.
- **Your settings and history** — your sort rules, categories, sender statistics and history, stored in an encrypted file in your own Google Drive app folder. You control this file and can delete it.
- **Delayed unsubscribe jobs** — when you reject a message with an unsubscribe link, we schedule the unsubscribe to run after a short delay so you can undo it. The job record holds the target and outcome, never your mail content, and is deleted after it completes.
- **Session records** — a session cookie and a server-side session record, with idle and absolute timeouts.
- **Pseudonymous operational data** — request IDs and pseudonymous identifiers in logs, kept for 90 days for security and reliability.

**Where your data is held.** All server-side data — our database (Firestore), encryption keys (Cloud KMS), secrets (Secret Manager), logs, and calls to the AI classifiers — is held in the **United States** (Google Cloud region `us-central1`). This is a cross-border disclosure of your information outside Australia under the Australian Privacy Principles (APP 8). By using Mail Tinder you consent to your information being held and processed in the United States.

**Who we share data with.** We share data only with the providers needed to run the service:

- **Google** (Gmail API, Google Drive, and Google Cloud) — to read and manage your mail, store your encrypted settings file, and host the service.
- **Unsubscribe targets** — when you reject a message with an unsubscribe link, we send the unsubscribe request to the sender's unsubscribe service, or send an unsubscribe email from your mailbox, exactly as you asked.
- **AI classifiers (optional, with your consent)** — see "Experimental classifiers" below.

We do not share your data with any analytics or advertising vendors.

**Experimental classifiers (optional).** You can opt in to an experimental feature that compares two AI classifiers on your mail. If you opt in, for each card we send a limited, redacted description — the sender's name and domain, the subject, and the first part of the message text with URLs, email addresses and long numbers replaced — to **Google (Vertex AI, United States)** and **TypeSafe AI (United States)**. Neither trains on your data. TypeSafe has not yet committed to how long it keeps this data and has no data processing agreement or security attestation in place. Anonymous accuracy figures from your swipes may be published. Anonymous totals already published or saved stay as they are if you later opt out. This feature is off by default. Turning it off stops all calls immediately and deletes the records we kept for it.

**Your rights.** You can:

- **Disconnect a mailbox** at any time, which revokes our access at the provider and deletes the mailbox record.
- **Delete your account** at any time, which deletes your account, your settings file, your tokens and every server record we hold about you.
- **Opt out** of the experimental classifiers at any time.

**Security.** Your mail access tokens are encrypted with keys held in Google Cloud KMS, and only our three server services can use them. We keep no backups of our database during the trial, so deleting your data deletes it. We log only pseudonymous operational data, never your mail content.

**Contact.** For privacy questions or to make a request, contact [CONTACT EMAIL] `[ASSUMES]`. If you are not satisfied with our response, you may complain to the Office of the Australian Information Commissioner (OAIC).

## 3. Google Limited Use statement

This statement is required for Google OAuth verification (S4 5.8, CR-01 2). It is the basis of the "Limited Use" disclosure Google requires for apps that use Gmail and Drive scopes.

### Google API Services User Data Policy — Limited Use statement

Mail Tinder's use of information received from Google APIs will comply with the **Google API Services User Data Policy**, including the **Limited Use requirements**. In particular:

1. **Limited to the user-facing purpose.** Mail Tinder uses Gmail and Drive data only to provide the user-facing features the user requested: reading and managing the user's mail, storing the user's encrypted settings file, and sending unsubscribes the user asked for. We do not use the data for any other purpose.
2. **Consent.** We obtain the user's consent before accessing their data, and we request only the minimum scopes needed: `gmail.modify`, `gmail.send`, `drive.appdata`, `openid` and `email` (S6 4). The user can revoke access at any time by disconnecting a mailbox or deleting their account.
3. **No training.** We do not use Gmail or Drive data to train any model. The optional experimental classifiers receive a limited, redacted description of a card only with the user's explicit opt-in consent, and neither Google's classifier nor TypeSafe trains on it (CR-01 2).
4. **No transfer for advertising.** We do not transfer Gmail or Drive data to any third party for advertising, ad targeting, or any purpose other than providing the user-facing feature.
5. **Human review.** We do not allow humans to read Gmail or Drive data except as needed for security, abuse prevention, or with the user's explicit consent, and never for building or improving models.
6. **Aggregate publication only.** Any published accuracy figures are aggregates with small-cell suppression, never examples from a user's mail (CR-01a).

## 4. Google OAuth verification pack

The materials the Google OAuth verification submission needs. `[ASSUMES]` marks items that require James to supply a value or document not present in the existing specs.

| # | Material | Source / status |
| --- | --- | --- |
| 1 | App name and description | Mail Tinder — swipe-to-triage email client. From S2/S9. |
| 2 | Privacy notice (section 2) | This spec. |
| 3 | Google Limited Use statement (section 3) | This spec. |
| 4 | OAuth consent screen configuration | gcp-setup.md "Outstanding (Phase B)": app name, owner email, test user. James to complete. |
| 5 | OAuth client (Web application) with exact redirect URIs | gcp-setup.md: `http://localhost:8080/oauth2callback` plus the Cloud Run URL. James to create. |
| 6 | Scopes requested and justification | S6 4: `gmail.modify`, `gmail.send`, `drive.appdata`, `openid`, `email`. Each scope maps to a user-facing feature. |
| 7 | Data handling and retention description | S5: no mail content at rest; retention per collection; deletion paths. |
| 8 | Security assessment evidence | S6 (ASVS Level 2 register), S10 (test strategy), S11 (operations). |
| 9 | TypeSafe (Jev) vendor terms | **Gate:** before Google verification, TypeSafe must supply a DPA, a retention commitment and a security attestation, or Jev is switched off and its code removed from the verified build (S4 5.8, CR-01 2). `[ASSUMES]` — the DPA/retention/attestation are not in the repo. |
| 10 | Contact and support details | `[ASSUMES]` — James to supply. |
| 11 | App icon and branding | `[ASSUMES]` — not in the specs. |
| 12 | Verification of the app's domain and ownership | `[ASSUMES]` — James to supply the domain and verification method. |

## 5. Bake-off consent dependency

The bake-off consent (CR-01 2, CR-01a G6) depends on this spec. The consent text shown in Settings, Experiments (S9) must name the recipients and what is sent, and must include the disclosure that TypeSafe has not yet committed to how long it keeps the data and has no DPA or security attestation (S4 5.8, CR-01 2, S10 BAKE-7). The privacy notice in section 2 carries the same disclosures, so the consent screen and the privacy notice stay consistent. The expected strings live in one fixture so a region change is a one-line edit (S10 BAKE-7).

## 6. Open questions

- **Company name, contact email, effective date** for the privacy notice — not in the existing specs. `[ASSUMES]` placeholders in section 2.
- **TypeSafe DPA, retention commitment and security attestation** — required before Google verification (S4 5.8); not in the repo.
- **App icon, branding, domain and ownership verification** for the Google pack — not in the specs.
- **Whether the privacy notice needs a separate Australian Privacy Policy document** beyond the notice text — not specified; James to confirm the format for the OAIC and Google.
