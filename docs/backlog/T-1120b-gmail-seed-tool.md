# T-1120b: Gmail seed, reset and verify tool for synthetic test mailboxes

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M11 | sonnet | medium: about 400 lines of Python (stdlib) plus tests | T-1120a, T-1106 |

**Read only these spec sections:** `docs/backlog/T-1106-nightly-live-gmail-contract-run.md` (as amended by PR #92, 11 Oct 2026: the live account is seeded by `messages.import`, never by sending; `tools/gmail_sandbox_reauth.py` mints the token). Until #92 is merged this task must not start (the picker's dependency on T-1106 enforces that), `docs/backlog/T-1120a-synthetic-mailbox-generator.md` (manifest format), `backend/crates/adapters-gmail/src/scopes.rs` (scope names). Nothing else is needed.

## Why this task exists

The owner decided (11 Oct 2026) that staging is tested with dedicated test Gmail accounts holding synthetic mail, that there will be several demo accounts, and that any account can be put back to a known state with one command. Gmail has no safe bulk delete for our scopes (the adapter has no delete, INV-5), so reset restores STATE, it does not wipe and reload.

## Goal

`python3 tools/gmail_seed.py --account NAME --manifest DIR/manifest.jsonl (seed|reset|verify) [--apply]` where DIR is a T-1120a output. Default is a dry run that prints what it would do.

## Files

| Action | Path | What |
| --- | --- | --- |
| Create | `tools/gmail_seed.py` | the tool (stdlib only, like `gmail_sandbox_reauth.py`) |
| Create | `tools/synthetic_accounts.json` | registry: `{"accounts": {"<name>": {"persona": "...", "seed": 1, "slice": "demo", "address_sha256": "<hex>", "corpus_sha256": "<hex>"}}}`; NO addresses, only the SHA-256 of the lowercased address |
| Create | `tools/test_gmail_seed.py` | unit tests against a local fake Gmail server (stdlib `http.server`) |
| Change | `tools/ac_coverage_enforced.txt` | add `T-1120b` |

## Algorithm

**Scopes and the no-send boundary.** The tool requests `gmail.modify`, `openid` and `email` only. Google accepts `gmail.modify` for `messages.send` too, so a missing `gmail.send` scope is NOT the boundary: the tool contains no send code path and its HTTP layer REFUSES any request whose path ends in `/messages/send` or `/drafts/send`, which a test proves. If Google rejects `messages.import` with these scopes, STOP and report; do not widen scopes on your own.

Credentials come from the environment (`GMAIL_SEED_CLIENT_ID`, `GMAIL_SEED_CLIENT_SECRET`, `GMAIL_SEED_REFRESH_TOKEN`); a workflow maps the per-account GitHub secret into these names, so the tool never knows which account's secret it holds. Mint an access token (`grant_type=refresh_token`; `invalid_grant` exits with its own code and message "needs re-authorising").

0. **Guard first.** Validate the whole manifest before any write: the header object must match the registry entry (`persona`, `seed`, `slice` AND `corpus_sha256`, the approved corpus digest, which the owner records in the registry when he approves a corpus; the tool never trusts a digest that only the manifest supplies, and it RECOMPUTES the digest from the files before any write using the exact rule in T-1120a (header-stripped `.eml` files plus the manifest body) and refuses on any mismatch) for `--account`, every `Message-ID` must match the generator's `<synth-...@reserved-host>` shape, and every file must carry `X-MT-Synthetic-Corpus` equal to the header's `corpus_hash`. Then call `users.getProfile`, lowercase the address, hash it, and refuse (exit 2, no other call) unless the hash equals the registry entry. Refuse any manifest message flagged `hostile: true` (T-1120a). Output is COUNTS, OUTCOME CODES and the account NAME only. Never print an address, token, message body, subject, Gmail message id, thread id or RFC 822 Message-ID, on any stream or error path.
1. **seed:** make sure the label `MT Synthetic` exists (`labels.list`, `labels.create` once). For each manifest message not already present (look up by RFC 822 id: `messages.list` with `q=rfc822msgid:<id>` AND `includeSpamTrash=true`, following every page), call `users.messages.import` with the raw RFC 822 bytes (base64url), `internalDateSource=dateHeader`, `neverMarkSpam=true`, `labelIds` mapped from the manifest labels (`INBOX`, `UNREAD`, plus `MT Synthetic`). Batch with a pause and exponential back-off on 429 and 5xx. Idempotent: a second run imports nothing.
2. **reset:** for each manifest message found by RFC 822 id (`includeSpamTrash=true`, all pages, `format=metadata` reading only `Message-ID` and `X-MT-Synthetic-Corpus`): require exactly ONE match and that it carries the matching `X-MT-Synthetic-Corpus` header; zero matches are `missing`, several or a missing/foreign header are `ambiguous` and are skipped and counted, never modified. If the `MT Synthetic` label has gone missing, recreate it and re-apply it ONLY to messages that carry the header. Provenance is bound to what THIS tool did, not to a header anyone could forge: `seed` writes the Gmail ids it imported to a receipts file (`~/.local/state/mt-seed/<account>.json`, mode 600, outside git, never printed). `reset` and `verify` act only on messages whose Gmail id is in the receipts AND that carry the matching header and label. If the receipts file is missing, the tool refuses to modify anything unless run once with `--adopt`, which accepts only messages whose Message-ID shape, header and label all match the registry digest, and then writes the receipts. Say plainly: `--adopt` is HEADER TRUST, not proof of origin; it is a one-time, interactive step (it prints the counts and requires typing the account name), and anyone who can write to the mailbox can plant a message that passes it. The dedicated-test-account decision contains that risk; the tool must not claim otherwise. In the workflow case (an ephemeral runner never has the receipts file) `reset` and `verify` always run the `--adopt` check against the registry digest and `MT Synthetic` label, and the spec accepts that weaker guarantee for the workflow path instead of uploading receipts anywhere. For each accepted message: if in trash, `messages.untrash`; then `messages.modify` so its label set equals the manifest baseline (add `INBOX` and `UNREAD` as the manifest says, remove `TRASH`, `SPAM` and any non-baseline label, keep `MT Synthetic`). Never delete. Messages missing from the mailbox are reported; `--reimport-missing` imports them. Messages in the account NOT in the manifest are counted as `extra`, never touched and never listed.
3. **verify:** compare the mailbox to the manifest (present, label set, trashed or not; same lookup rules as reset) and print a JSON summary of COUNTS only (`present`, `missing`, `drifted`, `ambiguous`, `extra`); exit 1 on any drift.

## Behaviours that must hold

| # | Behaviour |
| --- | --- |
| B1 | Without `--apply`, no mailbox-mutating request is made: the fake records no label-create, import, untrash, modify, delete or send call (the OAuth token exchange is allowed) |
| B2 | An unknown account name, or a profile whose address hash is not in the registry, exits 2 before any mailbox call other than `getProfile` |
| B3 | seed twice imports each message once |
| B4 | reset after the fake server changed labels, trashed messages and read some restores the baseline exactly; no delete endpoint is ever called |
| B5 | verify exits 1 on drift and 0 after reset |
| B6 | 429 and 5xx responses are retried with back-off; `invalid_grant` gives its own exit code |
| B7 | No output line on stdout, stderr or any error path contains the token, the address, a subject or body, a Gmail message id, a thread id or an RFC 822 Message-ID (tests scan all streams for each of these classes) |
| B8 | The fake excludes spam and trash from `messages.list` unless `includeSpamTrash=true`; reset finds and restores trashed messages without importing |
| B9 | A message with the right Message-ID but no matching header, or not in the receipts, is never modified; a manifest whose header or `corpus_sha256` disagrees with the registry is refused before any write; a forged header on a message not in the receipts is not touched without `--adopt`, and with `--adopt` it is accepted on header trust only (documented, not hidden) |
| B10 | A request to a `/messages/send` or `/drafts/send` path is refused by the tool's HTTP layer |

## Tests that must pass

`tools/test_gmail_seed.py` for B1 to B10 against a local fake server (no network, no credentials), plus all required checks. The first live run against the real test account is the owner-gated acceptance, recorded in the PR after he has finished the setup in `docs/ONTASK1`/the runbook; Google's documentation states `gmail.modify` is accepted for `messages.import`; if the live call is rejected, stop and report.

## Edge cases and traps

- Use only the official Gmail REST API with the account owner's consent; never send mail (this tool has no send code path and the scopes exclude `gmail.send`).
- The registry holds hashes, not addresses, so the repository never contains a test account's address.
- Keep the access token in memory only; never write it to disk or a log.
- Do not add a workflow here: `seed-staging-mailbox.yml` (inputs: account, mode, slice) is a separate owner-merged follow-up.

## Out of scope

The workflow, creating accounts, Outlook, attachments, deleting anything.

## Done when

The tests above pass, every required check is green (S10 10.1), definition of done in S10 10.4.
