# Runbook: the first admin

There is no default admin, and nobody can sign in without an invite
(ASVS V6.3.2, S7 3.7). James does this by hand, from his own machine, with the
`mt-admin` tool (`backend/crates/admin-cli`). The API never grants the admin
flag.

## Before you start (once)

1. `gcloud auth application-default login` — the tool reads Application Default
   Credentials through the `gcloud` CLI.
2. The tool needs your account to hold, on the project you pass with
   `--project`: Firestore read/write, Secret Manager **accessor** for
   `email-lookup-hmac-key` and `log-pseudonym-hmac-key`, and Cloud KMS
   **encrypt** on the `system-fields` key. It never needs the per-user KEK.
3. Build it once from the repo:

   ```sh
   cd backend
   cargo build --release -p admin-cli
   # binary: backend/target/release/mt-admin
   ```

## The three steps

All commands take `--project <id>`. Add `--origin https://...` only if the app is
not at the default `https://mailtinder.app`.

1. **Invite yourself.** `mt-admin --project <id> invite you@yourdomain.example`
   prints one line: the invite link. Open it in a browser and sign in with your
   Google account; the invite is single-use and expires after 7 days
   (AU-01 AC1). The link is the only place the token appears — paste it
   somewhere private, never into a chat or a ticket.

2. **Find your user ID.** `mt-admin --project <id> list-users` prints one line
   per user: user ID, created-at, whether they are an admin, and how many
   mailboxes they have. Listing never prints an email address. Copy your own
   user ID.

3. **Make yourself admin.** `mt-admin --project <id> make-admin <user_id>` sets
   the flag. It prints `admin granted` (or `already admin` if it was already
   set). Run `list-users` again to confirm exactly one admin.

Exit codes: `0` success, `2` bad input (bad address or user ID), `3` not found,
`1` anything else. Errors print one line to stderr and never include an email
address or token.

## Notes

- Revoking admin is out of scope for the trial: edit the user's document by hand
  in the Firestore console.
- The tool sends no email; you paste the link to yourself.
- Never store or log the invite link beyond the step above.