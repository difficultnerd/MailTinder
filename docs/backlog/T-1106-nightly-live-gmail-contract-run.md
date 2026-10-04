# T-1106: Nightly live Gmail contract run

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M11 | sonnet | about 300 lines of code plus a workflow | T-203, T-401, T-403, T-1103 |

**Read only these spec sections:** S10 sections 4.3 ("Live sandbox run" and "Shape fixtures") and 10.3, and decision T1 in section 11 (`docs/specs/S10-test-strategy.md`); S4 section 1 "Environments" (sandbox mailboxes) (`docs/specs/S4-architecture.md`); the "Backlog seeds for S12" line on the Gmail sandbox mailbox in `docs/planning-roadmap.md`. Nothing else is needed.

## Goal

Every night the shared `MailProvider` contract suite (T-203) runs against the real Gmail adapter using a dedicated sandbox Gmail account that holds only synthetic mail the suite sends to itself. Expired credentials are reported as their own outcome, not as a pass or a contract failure. James has the one-time setup and a weekly re-authorisation step with a helper script.

## Files

| Action | Path | What |
| --- | --- | --- |
| Create | `backend/crates/adapters-gmail/tests/live_contract.rs` | Live run, `#[ignore]`, behind feature `live` |
| Change | `backend/crates/adapters-gmail/Cargo.toml` | Feature `live = []` |
| Create | `backend/crates/adapters-gmail/tests/live_support/mod.rs` | Token minting, `LiveSeeder`, outcome classification |
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

/// Seeds synthetic corpus messages by sending them from the sandbox to itself (gmail.send), then waits for them in INBOX.
pub struct LiveSeeder { /* real adapter + address */ }
// Implements T-203's seeding hook for the contract suite (use T-203's trait name; if T-203 has none, add
// `trait MailSeeder { async fn seed(&self, fixture_id: &str) -> Result<MessageId, MailError>; }` in testkit and report it).
```

## Algorithm

1. `live_contract.rs` (`#[cfg(feature = "live")]`, `#[ignore = "nightly live run"]`): build `SandboxCreds`; missing variables give `NotConfigured` and the test fails with that message.
2. Mint an access token; `invalid_grant` gives `CredentialsExpired`: print `::error title=credentials expired::Gmail sandbox needs re-authorising` and exit with a failure.
3. Build the real Gmail adapter (T-401, T-403) with a real `HttpEgress` allowlisting only Gmail and Google OAuth hosts, then run `testkit::contract::mail_provider` with `LiveSeeder`.
4. Seeding sends corpus messages from T-204 (reserved `example.*` domains in display text, addressed to the sandbox itself) and tags them with the fixed label `MT Live Contract` `[DEFAULT]` so the suite only touches its own messages.
5. Cleanup: trash every message the run created (no delete method exists, INV-5; Gmail empties trash after 30 days). Labels are reused across runs, never created per run, because the adapter cannot delete labels.
6. **Workflow** `live-contract.yml`: `schedule: cron "23 17 * * *"` `[DEFAULT]` and `workflow_dispatch`; `environment: live-sandbox` (holds the four secrets); `cargo test --locked -p adapters-gmail --features live --test live_contract -- --ignored --nocapture`. On failure, a step with `permissions: issues: write` opens or updates one issue: title "Gmail sandbox needs re-authorising" when the log has the credentials-expired annotation, otherwise "Live Gmail contract run failed" with a link to the run. Not added to required checks.
7. **Reauth helper** `tools/gmail_sandbox_reauth.py` (stdlib): reads client ID and secret from environment, starts a loopback server on `127.0.0.1:0`, opens the browser to Google's authorisation URL with PKCE, `access_type=offline`, `prompt=consent`, and the S6 Gmail scopes (`gmail.modify`, `gmail.send`, `openid`, `email`), exchanges the code, and pipes the refresh token into `gh secret set GMAIL_SANDBOX_REFRESH_TOKEN --env live-sandbox` through stdin. It never prints or writes the token.
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
- Never log tokens, the sandbox address or message content: wrap them in `Sensitive` and keep `--nocapture` output to check names.
- Never call a delete endpoint; trash only.
- The job must not run on pull requests or forks.
- A credentials-expired run is a failure with its own title, never a silent pass.
- When the live run finds a difference from the fakes, the fix is a new hand-written fixture in the HTTP fake plus a contract assertion, never a captured response (S10 4.3).

## Out of scope

- The Outlook.com sandbox (v2 with Microsoft).
- Drive app folder live contract (`AppFolderStore`) `[DEFAULT]`: add when T-405's contract suite exists; needs `drive.appdata` in the reauth scopes.

## Done when

- The tests above pass and every required check is green (S10 10.1).
- Definition of done in S10 10.4.
- James has completed the setup steps and one nightly run has passed.
