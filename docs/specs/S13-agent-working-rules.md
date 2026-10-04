# S13: Agent Working Rules

Status: DRAFT for James's review. 3 October 2026.
Audience: the build threads, which run on a lower-cost model (James, 3 October 2026), and James as reviewer.
Depends on: `docs/backlog/README.md` (S12), `docs/backlog/CONVENTIONS.md`, `S10-test-strategy.md` section 10.

## 1. Who does what

| Role | Model tier | Does |
| --- | --- | --- |
| Planning thread | Strong | Owns the specs, the backlog and `CONVENTIONS.md`. Answers spec questions. Reviews every strong-tier task before merge |
| Build thread | Sonnet by default | Builds one task at a time from its task file. Never edits a spec |
| James | Human | Merges pull requests, reviews Terraform plans, does the steps marked "James" (branch protection, sandbox mailbox, secrets) |

A task tagged **strong** is built by a strong-tier thread, or by a build thread whose pull request then gets a strong-tier review against the task's "Security review checklist" before James merges it.

## 2. Picking a task

1. Open `docs/backlog/README.md`. Pick the lowest-numbered task whose dependencies are all merged to `main` and which has no open pull request (search open pull requests for its ID).
2. Prefer the task James or the planning thread named, if any.
3. Do not start a task marked "needs S11" until S11 exists.
4. One task per thread and per pull request. If two tasks are free, two threads may run in parallel, as long as neither changes a file the other's task file lists.

## 3. Before writing code

1. Read, in order: the task file, `CONVENTIONS.md`, and only the spec sections the task file names. Do not read the rest of the spec set; it costs tokens and invites scope creep.
2. Create the branch `task/T-xxx-short-name` from the latest `main`.
3. If the task file is ambiguous, contradicts a spec, or needs a shared type that does not exist, stop and ask the planning thread (through the coordinator). Do not guess and do not edit the spec or `CONVENTIONS.md` yourself.

## 4. Building

1. **Tests first.** Write the tests listed under "Tests that must pass", named exactly as listed, and watch them fail.
2. Write the smallest code that makes them pass, following "Types and signatures" and "Algorithm". Keep public names as written.
3. Stay inside the "Files" table. A file not listed needs a one-line reason in the pull request.
4. Follow "Edge cases and traps". They are there because a builder got them wrong before, or would.
5. No new dependency unless the task file names it. No network access in tests except the local fakes.
6. Never add `#[ignore]`, `skip`, a lowered coverage floor, or an `allow` attribute to get green.

## 5. Checking before a pull request

Run, and fix everything they report:

```
cd backend && cargo fmt --all && cargo clippy --all-targets --all-features --locked -- -D warnings && cargo test --all-features --locked
cd app && dart format . && flutter analyze --fatal-infos && flutter test
tools/check_language_policy.sh
python3 tools/check_ac_coverage.py      # once T-002 is merged; add your task ID to tools/ac_coverage_enforced.txt
```

Then show the tests guard the behaviour: revert the core change locally, run the tests, and note one failing test name for the pull request (S10 10.4 item 1).

## 6. Pull request

- Title: `T-xxx: <task title>`.
- Body: use the repository's pull request template if one exists. Otherwise: "Before:" and "After:" paragraphs in plain words, the AC IDs covered, the failing test name from the revert check, any file changed outside the task's list and why, and anything the reviewer should look at first.
- Open it as a draft, mark it ready when every required check is green (S10 10.1). No retries to get green; a flaky test is a bug to fix (S10 10.5).
- A strong-tier task also pastes its "Security review checklist" with each item ticked or explained.

## 7. When the build model is stuck

Escalate to the planning thread, through the coordinator, after two failed attempts at the same check, or at once for any of these: a crypto, auth, egress or deletion question; a test that seems to need real mail or a real account; a spec that contradicts its task file. Say what was tried and paste the failing output. The planning thread either fixes the task file or takes the task over.

## 8. After merge

- The planning thread marks the task done in the backlog index and updates `CONVENTIONS.md` if the task added a shared type.
- Lessons that would have saved a builder time go into the next relevant task file's "Edge cases and traps".

## 9. Never

- Commit secrets, real email addresses or real mail content (Gitleaks and the corpus domain check enforce this).
- Call a provider's permanent delete (INV-5).
- Log a message body, subject, snippet, address or URL (XC-01).
- Edit a file under `docs/specs/` or `docs/security/` from a build thread.
- Rewrite history on a branch someone else owns.
