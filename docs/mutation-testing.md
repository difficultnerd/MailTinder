# Mutation testing (pilot)

Coverage says a line ran. Mutation testing says a test would notice if the line
were wrong: cargo-mutants edits the code in small ways (flip a comparison, swap
a return value, delete a statement) and then runs the test suite. A change the
suite does not notice is a **missed mutant** and marks a real gap.

This is a pilot on the highest-value pure logic, not a whole-repo gate. A full
run is far too slow for every push, so it is deliberately **not** part of
`tools/ci-local.sh`; it runs on demand and nightly on verify1.

## Scope

`tools/mutation_scope.txt` lists one area per line:

    rules: backend/crates/domain/src/rules.rs
    swipe: backend/crates/domain/src/swipe.rs
    ...

Only hand-written pure logic is listed. cargo-mutants already skips `#[test]`
functions, `#[cfg(test)]` modules, `#[mutants::skip]` and `unsafe` code, and the
run is restricted with `--file` to exactly the scope paths, so test crates,
adapters, the API crate and generated code are never mutated.

## Install

    ./tools/setup-local-checks.sh

This installs `cargo-mutants` v27.1.0 into `tools/.bin` on x86_64 Linux, pinning
the release tarball by SHA-256. Other architectures fall back to a pinned
`cargo install`. Put `tools/.bin` on your PATH (ci-local.sh already does).

## Run

    tools/mutation.sh                       # every area
    tools/mutation.sh --area rules          # one area
    tools/mutation.sh --file backend/crates/domain/src/rules.rs
    tools/mutation.sh --jobs 8              # parallel build/test jobs
    tools/mutation.sh --shard 0/8           # shard 0 of 8
    tools/mutation.sh --list                # print the selected files, run nothing

The script runs cargo-mutants from `backend/`, keeps its artifacts under
`target/mutants/` and then runs `tools/mutation_report.py`, which writes
`target/mutants/summary.json` and exits non-zero when an area is below its
floor. A path passed to `--file` that is not in `tools/mutation_scope.txt` is
rejected before anything runs.

### Sharding on verify1

verify1 has 8 cores. Split the work so a full pilot finishes inside an hour:

    for i in 0 1 2 3 4 5 6 7; do
      tools/mutation.sh --shard "$i/8" --jobs 1 &
    done
    wait

Each shard writes to `target/mutants/shard-Iof8/`; the per-shard report merges
every `outcomes.json` found under `target/mutants/`. All shards must use the
same arguments and the same denominator, or the split is meaningless. Record
the wall-clock duration of the run in the pull request.

A nightly run on verify1 is scheduled by the owner/operator (the factory does
not edit cron or workflows). Example crontab entry:

    15 2 * * * cd /srv/MailTinder && ./tools/setup-local-checks.sh >/dev/null \
      && ./tools/mutation.sh --jobs 8 >/tmp/mutation.log 2>&1

## Read the report

    tools/mutation_report.py --input target/mutants
    tools/mutation_report.py --input target/mutants --markdown   # paste into an issue

Per area it prints caught, missed, timeout, unviable, the score and the floor:

| Area | Caught | Missed | Timeout | Unviable | Score | Floor | Status |

Scoring is `(caught + timeout) / (caught + missed + timeout)`. A mutant that
times out counts as caught (it did not survive), so timeouts are folded into
the numerator; unviable mutants never compiled and are kept out of the score.
An area with no scored mutants has no score and cannot be below floor.

Every missed mutant is then listed with its file and line:

    - rules: crates/domain/src/rules.rs:42:5  replace guard with true

To understand one, open that line and look at what the mutation changed: the
diff is in `target/mutants/mutants.out/mutants.json` and the test log that
failed to catch it in `target/mutants/mutants.out/log/`. Then add an assertion
that fails for the mutated behaviour. For the three most important areas
(`rules`, `swipe`, `undo`) the pilot adds tests that kill at least five missed
mutants each, or states in one line why a mutant is equivalent.

## Baseline

`tools/mutation_baseline.json` holds the floor per area:

    { "rules": { "floor": 80.0 }, "swipe": { "floor": 80.0 }, ... }

The floor is the measured first-run score minus two points, so the score can
only go up. After the first full run, replace the provisional values shipped
here with `measured − 2`, using the numbers in that run's report (and quoted in
the pull request). A later run that drops below a floor exits non-zero.
