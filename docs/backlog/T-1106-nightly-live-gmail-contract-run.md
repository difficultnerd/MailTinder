# T-1106: Nightly live Gmail contract run

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M11 | sonnet | about 300 lines of code plus a workflow | T-203, T-401, T-403, T-1103 |

**Read only these spec sections:** S10 sections 4.3 ("Live sandbox run" and "Shape fixtures") and 10.3, and decision T1 in section 11 (`docs/specs/S10-test-strategy.md`); S4 section 1 "Environments" (sandbox mailboxes) (`docs/specs/S4-architecture.md`); the "Backlog seeds for S12" line on the Gmail sandbox mailbox in `docs/planning-roadmap.md`. Nothing else is needed.

## Goal

Every night the shared `MailProvider` contract suite (T-203) runs against the real Gmail adapter using a dedicated sandbox Gmail account that holds only synthetic mail that is inserted into it with the Gmail `messages.import` API (owner decision 2026-10-11: the account is never used to send seed mail). Expired credentials are reported as their own outcome, not as a pass or a contract failure. James has the one-time setup and a weekly re-authorisation step with a helper script.

## Files

| Action | Path | What |
| --- | --- | --- |
| Create | `backend/crates/adapters-gmail/tests/live_contract.rs` | Live run, `#[ignore]`, behind feature `live` |
| Change | `backend/crates/adapters-gmail/Cargo.toml` | Feature `live = []` |
| Create | `backend/crates/adapters-gmail/tests/live_support/mod.rs` | Token minting, `LiveSeeder` (import-based), outcome classification |
| Create | `.github/workflows/live-contract.yml` | Nightly and manual; not a required check |
| Create | `tools/gmail_sandbox_reauth.py` | Stdlib loopback OAuth helper that stores the refresh token with `gh secret set` |
| Create | `docs/runbooks/gmail-sandbox.md` | James's setup and weekly steps |

## Types and signatures

```rust
pub enum LiveOutcome { Passed, ContractFailed(String), CredentialsExpired, NotConfigured }

/// Reads GMAIL_SANDBOX_CLIENT_ID, GMAIL_SANDBOX_CLIENT_SECRET, GMAIL_SANDBOX_REFRESH_TOKEN, GMAIL_SANDBOX_ADDRESS.
pub struct SandboxCreds { pub client_id: String, pub client_secret: Sensitive<String>, pub refresh: Sensitive<String>, pub address: Sensitive<String> }
impl SandboxCreds { pub fn from_env() -> Result<Self, LiveError>; }

/// POST https://oauth2.googleapis.com/token grant_type=refresh_token; invalid_grant -> CredentialsExpired.
pub async fn mint_access_token(c: &SandboxCreds) -> Result<Sensitive<String>, LiveError>;

/// Seeds synthetic corpus messages by INSERTING them into the sandbox mailbox with
/// `users.messages.import` (raw RFC 822 from the T-204 corpus, `labelIds` and `internalDateSource=dateHeader`
/// taken from the `SeedMessage`). Nothing is sent; no `gmail.send` is used to seed. Returns the Gmail message id.
pub struct LiveSeeder { /* real adapter + address */ }
// Implements T-203's seeding hook for the contract suite (use T-203's trait name; if T-203 has none, add
// `trait MailSeeder { async fn seed(&self, fixture_id: &str) -> Result<MessageId, MailError>; }` in testkit and report it).
```

## Algorithm

1. `live_contract.rs` (`#[cfg(feature = "live")]`, `#[ignore = "nightly live run"]`): build `SandboxCreds`; missing variables give `NotConfigured` and the test fails with that message.
2. Mint an access token; `invalid_grant` gives `CredentialsExpired`: print `::error title=credentials expired::Gmail sandbox needs re-authorising` and exit with a failure.
3. Build the real Gmail adapter (T-401, T-403) with a real `HttpEgress` allowlisting only Gmail and Google OAuth hosts, then run `testkit::contract::mail_provider` with `LiveSeeder`.
4. Seeding IMPORTS corpus messages from T-204 (`SeedMessage` -> raw `.eml`; reserved `example.*` domains; `labels` and `internal_date` carried across) with `users.messages.import`, never `messages.send` or SMTP, and tags them with the fixed label `MT Live Contract` `[DEFAULT]` so the suite only touches its own messages. Import is idempotent per run: record the returned ids; a run that finds its own label already populated trashes those messages first. `labels_of` reads back with `messages.get`; `sent()` queries the sandbox for messages carrying the `SENT` label and returns them (it is never a constant empty list); see 'Live run case groups and the no-send boundary'.
5. Cleanup: trash every message the run created (no delete method exists, INV-5; Gmail empties trash after 30 days). Labels are reused across runs, never created per run (keeps the account tidy; the suite's own label-removal case works on a label it created).
6. **Workflow** `live-contract.yml`: `schedule: cron "23 17 * * *"` `[DEFAULT]` and `workflow_dispatch`; `environment: live-sandbox` (holds the four secrets); `cargo test --locked -p adapters-gmail --features live --test live_contract -- --ignored` with output captured to a file (see 'Edge cases': no `--nocapture`). On failure, a separate follow-up JOB with `permissions: { contents: read, issues: write }` (`permissions:` is valid only at workflow or job level, so the write scope must be confined to that job and kept off the job that holds the credentials) opens or updates one issue: title "Gmail sandbox needs re-authorising" when the log has the credentials-expired annotation, otherwise "Live Gmail contract run failed" with a link to the run. Not added to required checks.
7. **Reauth helper** `tools/gmail_sandbox_reauth.py` (stdlib): reads client ID and secret from environment, starts a loopback server on `127.0.0.1:0`, opens the browser to Google's authorisation URL with PKCE, `access_type=offline`, `prompt=consent`, and the scopes `gmail.modify`, `openid`, `email` (`gmail.modify` covers `messages.import` in the implementer's reading of Google's scope table: verify at the first live run and add `gmail.insert` only if Google rejects the import; `gmail.send` is NOT requested, because nothing here sends), exchanges the code, and pipes the refresh token into `gh secret set GMAIL_SANDBOX_REFRESH_TOKEN --env live-sandbox` through stdin. It never prints or writes the token.
8. **Steps for James** (in `docs/runbooks/gmail-sandbox.md`; not the builder's job):
   1. Create a new Gmail account used only for this; never forward real mail to it.
   2. In the staging project (T-1103), add it as a test user on the OAuth consent screen (Testing mode) and create a Desktop OAuth client for the live run.
   3. Create the GitHub environment `live-sandbox` and set `GMAIL_SANDBOX_CLIENT_ID`, `GMAIL_SANDBOX_CLIENT_SECRET`, `GMAIL_SANDBOX_ADDRESS`.
   4. Run `tools/gmail_sandbox_reauth.py` to set `GMAIL_SANDBOX_REFRESH_TOKEN`.
   5. Weekly (Testing mode refresh tokens last 7 days), or whenever the "needs re-authorising" issue opens: run step 4 again and close the issue.

## Acceptance criteria

None from S2: this task keeps the HTTP fakes honest (S10 4.3, decision T1). The contract suite's own IDs belong to T-203.

## Tests that must pass

- `live_contract` passes in a manual `workflow_dispatch` run after James's setup (link in the PR).
- Unit tests in `live_support`: `invalid_grant` maps to `CredentialsExpired`; missing variables map to `NotConfigured` (these run in normal CI without credentials).

## Edge cases and traps

- The sandbox holds synthetic mail only; the suite must act only on messages it created (fixed label plus IDs it recorded).
- Never log tokens, the sandbox address or message content. `Sensitive` does not redact strings the shared suite has already built: its order-mismatch errors interpolate message subjects and other errors interpolate provider error values (`backend/crates/testkit/src/contract/mail_provider.rs:128-131,159,304-310,376-377`). So the live job runs WITHOUT `--nocapture`, captures the test output to a file that is never printed or uploaded, and prints only the pass/fail result and the NAMES of failing tests (fixed identifiers) read from that file; the live fixtures keep the corpus's original synthetic subjects, because the unchanged shared suite compares returned subjects with the seeded values (`backend/crates/testkit/src/contract/mail_provider.rs:45-55,121-131,304-310,320-355,376-377`); a rewritten subject would fail the read group against a correct mailbox. Content-freeness therefore rests on never printing or uploading the captured output.
- The shared suite's factory contract requires a fresh, empty mailbox per call (`backend/crates/testkit/src/contract/mail_provider.rs:105-113`); the live factory must restore that isolation before every case (trash everything carrying the `MT Live Contract` label), not once per run.
- Never call a MESSAGE delete endpoint (`messages.delete`, `messages.batchDelete`, `threads.delete`); trash only. Label removal by the suite's own case is allowed (see 'Live run case groups').
- The job must not run on pull requests or forks.
- A credentials-expired run is a failure with its own title, never a silent pass.
- When the live run finds a difference from the fakes, the fix is a new hand-written fixture in the HTTP fake plus a contract assertion, never a captured response (S10 4.3).

## Live run case groups and the no-send boundary

This task SUPERSEDES the sentence in `docs/specs/S10-test-strategy.md` s.4.3 ("synthetic mail sent by the suite itself"): the live account is seeded by import and never sends. The rule, restated here so no private document is needed: the live sandbox account sends nothing to anyone; seed mail is inserted with `users.messages.import`; hostile or phishing-style content is never loaded into a real account.

The live run uses `CaseGroups { read: true, modify: true, send: false }` `[DEFAULT]`; the `send` group (`send_mailto`) would email a third party from a real account and stays proven by the fakes (T-203/T-205a).

**Dropping `gmail.send` from the scopes is hygiene, not prevention:** Google accepts `gmail.modify` for `users.messages.send` too (reviewer-verified against the provider reference; the fake test context holds only the modify scope and `mail_provider_contract_send_gmail` sends with it, `backend/crates/adapters-gmail/tests/contract_send.rs:243-259`). So the boundary is enforced in code and checked after every run:

1. **Transport guard (prevention).** The outer `MailProvider` is not the only path to a send: a delegated read or modify call could reach the send endpoint inside the adapter without passing a decorator. So the guard sits at the HTTP layer: the live `HttpEgress` (step 3) gets a request filter that REFUSES any request whose path ends in `/messages/send` or `/drafts/send` (and any `Subject`-less raw upload to them), records the attempt, and fails the run. A decorator on `send_mailto` may be added as a second layer but is not the boundary.
2. **Negative check (secondary detector; the transport guard in item 1 is the boundary, this only notices a send that got past it).** `LiveSeeder::sent()` queries the sandbox for messages carrying the `SENT` label (`messages.list` with `labelIds=SENT` and `includeSpamTrash=true`) and returns them; the run FAILS if the list is non-empty at the end (and the same query runs before the first case, to catch a polluted account). It must not return a constant empty list.
3. **Label removal is not message deletion.** The "never call a delete endpoint" rule means the MESSAGE delete endpoints (`messages.delete`, `messages.batchDelete`, `threads.delete`); the shared suite's own `remove_label_keeps_messages` case calls the adapter's label-removal path (`backend/crates/adapters-gmail/src/labels.rs:44-52`) and is allowed, on the label the suite created.

## Out of scope

- The Outlook.com sandbox (v2 with Microsoft).
- Drive app folder live contract (`AppFolderStore`) `[DEFAULT]`: add when T-405's contract suite exists; needs `drive.appdata` in the reauth scopes.

## Done when

- The tests above pass and every required check is green (S10 10.1).
- Definition of done in S10 10.4.
- James has completed the setup steps and one nightly run has passed.
