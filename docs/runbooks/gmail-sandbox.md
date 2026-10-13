# Gmail sandbox for the nightly live contract run (T-1106)

The nightly `live-contract` workflow (`.github/workflows/live-contract.yml`) runs
the shared `MailProvider` contract suite (T-203) against the real Gmail adapter
on a dedicated sandbox account. This runbook is what James does; the workflow
and the helper do the rest.

The sandbox holds synthetic mail only, and it never sends anything. Seed mail is
inserted with the Gmail `users.messages.import` API, the workflow's transport
guard refuses any call to a send endpoint, and the run fails if the account's
`SENT` label is not empty. Hostile or phishing-style content is never loaded
into a real account.

## One-time setup

1. **Create a new Gmail account used only for this.** Never forward real mail to
   it, and do not add it to any other service. The address goes in the
   `GMAIL_SANDBOX_ADDRESS` secret below.

2. **Register it with the staging project (T-1103).** In the Google Cloud
   staging project, on the OAuth consent screen (leave it in **Testing** mode),
   add the sandbox address as a **test user**. Then create an OAuth client of
   type **Desktop app** for the live run. Note its client ID and client secret.

3. **Create the GitHub environment `live-sandbox`** (repository Settings →
   Environments) and set three secrets on it:

   - `GMAIL_SANDBOX_CLIENT_ID`
   - `GMAIL_SANDBOX_CLIENT_SECRET`
   - `GMAIL_SANDBOX_ADDRESS`

   Restrict the environment to the `main` branch. The workflow runs only from
   `schedule` and `workflow_dispatch`, never on a pull request or a fork.

4. **Store the refresh token.** With the client ID and secret in the
   environment:

   ```
   export GMAIL_SANDBOX_CLIENT_ID=...
   export GMAIL_SANDBOX_CLIENT_SECRET=...
   python3 tools/gmail_sandbox_reauth.py
   ```

   The helper opens the browser, requests `gmail.modify`, `openid` and `email`
   with PKCE and `access_type=offline&prompt=consent`, and pipes the refresh
   token into
   `gh secret set GMAIL_SANDBOX_REFRESH_TOKEN --env live-sandbox` on stdin. It
   never prints or writes the token; it prints only the address it stored the
   secret for.

5. **Run the workflow once by hand** (Actions → `live-contract` → Run workflow)
   and confirm it passes before relying on the nightly schedule.

## Weekly, or when the issue opens

Google expires refresh tokens after 7 days while the consent screen is in
Testing mode. Do one of these:

- Weekly, re-run step 4 above; or
- when the workflow opens **"Gmail sandbox needs re-authorising"**, re-run step
  4 and then close the issue.

Re-run the workflow by hand afterwards to confirm the token works.

## What the workflow does

- `cargo test --locked -p adapters-gmail --features live --test live_contract -- --ignored`
  with the four secrets, captured to a file that is never printed and never
  uploaded (the shared suite interpolates subjects and provider error values in
  its own error strings). The step prints only the pass/fail result and the names
  of failing tests.
- The live job holds the credentials but has read-only permissions, and passes
  the fixed outcome code (`Passed`, `ContractFailed`, `CredentialsExpired`,
  `NotConfigured`) to a follow-up job as a job output.
- The follow-up job has `issues: write` and holds no credentials. It opens or
  updates one issue: **"Gmail sandbox needs re-authorising"** for expired
  credentials, otherwise **"Live Gmail contract run failed"** with a link to the
  run.

Neither job is a required check.

## When the live run differs from the fakes

The fix is a new hand-written fixture in the HTTP fake plus a contract
assertion, never a captured response (S10 4.3). Capturing a real response would
put real mailbox content in the repository.
