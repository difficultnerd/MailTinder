# T-1108b: demo seed: a fake mailbox and a one-line invite URL

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M11 | sonnet | about 80 lines of code plus tests | T-1108a |

**Read only these spec sections:** the files named under Files and the task files in Depends on. Nothing else is needed.

## Goal

After `demo.sh start`, a seeded fake account with the synthetic mail corpus exists and the script prints the invite URL, so opening it signs in and shows a full Feed.

## Files

| Action | Path | What |
| --- | --- | --- |
| Create | `scripts/demo_seed.py` | Stdlib only: seeds `invitee@example.com` with the corpus via the fake-google control API and creates an invite through the e2e invite route; prints the URL |
| Change | `scripts/demo.sh` | Call the seed after start and print the URL |

## Behaviour

1. Seeding is idempotent (running twice does not duplicate mail).
2. The synthetic corpus includes newsletters, promos, personal mail and a one-click unsubscribe sender so every swipe type has something to act on.

## Acceptance criteria (each a named test that fails if the behaviour is removed)

- `demo_seed_creates_expected_messages`
- `demo_seed_is_idempotent`
- `demo_check_feed_shows_seeded_sender`

## Out of scope

Phone access, hot reload.

## Done when

PR body shows the printed URL (token redacted) and the Feed's first card text. Definition of done in S10 10.4. Do not edit `.github/workflows/` or `CLAUDE.md`.
