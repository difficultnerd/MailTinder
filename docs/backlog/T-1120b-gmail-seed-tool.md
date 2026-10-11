# T-1120b: Gmail seed, reset and verify tool for synthetic test mailboxes

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M11 | sonnet | medium: about 400 lines of Python (stdlib) plus tests | T-1120a, T-1106 |

**Read only these spec sections:** `docs/backlog/T-1106-nightly-live-gmail-contract-run.md` (as amended 11 Oct 2026: seeding is by import; `tools/gmail_sandbox_reauth.py` mints the token), `docs/backlog/T-1120a-synthetic-mailbox-generator.md` (manifest format), `backend/crates/adapters-gmail/src/scopes.rs` (scope names). Nothing else is needed.

## Why this task exists

The owner decided (11 Oct 2026) that staging is tested with dedicated test Gmail accounts holding synthetic mail, that there will be several demo accounts, and that any account can be put back to a known state with one command. Gmail has no safe bulk delete for our scopes (the adapter has no delete, INV-5), so reset restores STATE, it does not wipe and reload.

## Goal

`python3 tools/gmail_seed.py --account NAME --manifest DIR/manifest.jsonl (seed|reset|verify) [--apply]` where DIR is a T-1120a output. Default is a dry run that prints what it would do.

## Files

| Action | Path | What |
| --- | --- | --- |
| Create | `tools/gmail_seed.py` | the tool (stdlib only, like `gmail_sandbox_reauth.py`) |
| Create | `tools/synthetic_accounts.json` | registry: `{"accounts": {"<name>": {"persona": "...", "seed": 1, "slice": "demo", "address_sha256": "<hex>"}}}`; NO addresses, only the SHA-256 of the lowercased address |
| Create | `tools/test_gmail_seed.py` | unit tests against a local fake Gmail server (stdlib `http.server`) |
| Change | `tools/ac_coverage_enforced.txt` | add `T-1120b` |

## Algorithm

Credentials come from the environment (`GMAIL_SEED_CLIENT_ID`, `GMAIL_SEED_CLIENT_SECRET`, `GMAIL_SEED_REFRESH_TOKEN`); a workflow maps the per-account GitHub secret into these names, so the tool never knows which account's secret it holds. Mint an access token (`grant_type=refresh_token`; `invalid_grant` exits with its own code and message "needs re-authorising").

0. **Guard first.** Call `users.getProfile`, lowercase the address, hash it, and refuse (exit 2, no other call) unless the hash equals the registry entry for `--account`. Refuse any manifest message flagged `hostile: true` (T-1120a). Never print an address, token or message body: output is ids, counts and the account NAME only.
1. **seed:** make sure the label `MT Synthetic` exists (`labels.list`, `labels.create` once). For each manifest message not already present (look up by RFC 822 id: `messages.list` with `q=rfc822msgid:<id>`), call `users.messages.import` with the raw RFC 822 bytes (base64url), `internalDateSource=dateHeader`, `neverMarkSpam=true`, `labelIds` mapped from the manifest labels (`INBOX`, `UNREAD`, plus `MT Synthetic`). Batch with a pause and exponential back-off on 429 and 5xx. Idempotent: a second run imports nothing.
2. **reset:** for each manifest message found by RFC 822 id: if in trash, `messages.untrash`; then `messages.modify` so its label set equals the manifest baseline (add `INBOX` and `UNREAD` as the manifest says, remove `TRASH`, `SPAM` and any non-baseline label, keep `MT Synthetic`). Never delete. Messages missing from the mailbox are reported; `--reimport-missing` imports them. Messages in the account NOT in the manifest are reported, not touched.
3. **verify:** compare the mailbox to the manifest (present, label set, trashed or not) and print a JSON summary (`present`, `missing`, `drifted`, `extra` counts and up to 20 ids each); exit 1 on any drift.

## Behaviours that must hold

| # | Behaviour |
| --- | --- |
| B1 | Without `--apply`, no write call is made (the fake server records zero POST/modify) |
| B2 | An unknown account name, or a profile whose address hash is not in the registry, exits 2 before any mailbox call other than `getProfile` |
| B3 | seed twice imports each message once |
| B4 | reset after the fake server changed labels, trashed messages and read some restores the baseline exactly; no delete endpoint is ever called |
| B5 | verify exits 1 on drift and 0 after reset |
| B6 | 429 and 5xx responses are retried with back-off; `invalid_grant` gives its own exit code |
| B7 | No output line contains the token, the address, or any message subject or body (tests scan stdout and stderr) |

## Tests that must pass

`tools/test_gmail_seed.py` for B1 to B7 against a local fake server (no network, no credentials), plus all required checks. The first live run against the real test account is the owner-gated acceptance, recorded in the PR after he has finished the setup in `docs/ONTASK1`/the runbook; it also settles whether `gmail.modify` alone allows `messages.import` (if Google rejects it, add `gmail.insert` to the reauth scopes in a follow-up and say so).

## Edge cases and traps

- Use only the official Gmail REST API with the account owner's consent; never send mail (this tool has no send code path and the scopes exclude `gmail.send`).
- The registry holds hashes, not addresses, so the repository never contains a test account's address.
- Keep the access token in memory only; never write it to disk or a log.
- Do not add a workflow here: `seed-staging-mailbox.yml` (inputs: account, mode, slice) is a separate owner-merged follow-up.

## Out of scope

The workflow, creating accounts, Outlook, attachments, deleting anything.

## Done when

The tests above pass, every required check is green (S10 10.1), definition of done in S10 10.4.
