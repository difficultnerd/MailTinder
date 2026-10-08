# T-802 verification notes

Acceptance criteria: ST-02 AC1, ST-02 AC2, ST-02 AC3, GM-05 AC2,
GM-06 AC1, GM-06 AC2, GM-06 AC3.

`Totals.levels_cleared` is a serde-defaulted list of completed years. Progress
uses it to count each completed year once, including after retries.

Security review follow-up:

- F1: The block branch of POST /api/v1/rules opens a server-issued prompt bound
  to the authenticated user and session, verifies mailbox ownership, and calls
  the shared block-rule service. The achievement test now obtains the prompt
  through real rejects and drives the HTTP endpoint twice.
- F2: Accepted bounded risk: GET /progress may persist idempotent level
  completion. The CSRF middleware checks unsafe methods only; SameSite=Lax
  does not prevent a cross-site top-level GET. This is not a claim that GET
  has CSRF protection. Completion is computed from the user's existing state,
  without attacker-selected mutation parameters, and cannot count a year twice.
  This exception is explicitly permitted by T-802. GET /stats remains read-only.
- F3: Unknown stored achievement IDs are omitted from the response and retained
  in state for compatibility with other builds.
- F4: Filing creates the category, increments the counter, records achievements
  and caches the response in the swipe update. Failed provider labeling does
  not persist a category or consume an unlock; retry returns the unlock.
- F5: Swipe and reject persistence use the same fallible update-once helper;
  its duplicate guard runs again on ETag conflict. Failed closure mutations
  are discarded.

The block endpoint added here implements only the block creation branch needed
by T-802, not the remaining T-608 rules operations.

Mutation evidence: removing achievement persistence from `record_unlocks`
made `gm_06_ac1_first_blocked_person_on_block_rule` fail (exit 101). The mutation
was restored before the final green checks.

Verified checks: Rust formatting, Clippy (all targets/features, locked, warnings
denied), full Rust tests, Dart formatting, Flutter analysis with fatal infos,
Flutter tests, AC coverage, Rust and Flutter coverage generation, and coverage
floors. `tools/ac_coverage_enforced.txt` already contains T-802 exactly once.
