# T-1118: Owner-input helpers and console checklist (the steps that must stay human)

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M11 | sonnet | about 120 lines of bash plus a runbook | T-1102a |

**Read only these spec sections:** `docs/decisions/0003-infrastructure-changes-owner-decides-pipeline-applies.md` (Decision 5); `infra/terraform/README.md` step 5 (secret names); `docs/runbooks/` existing files for style. Nothing else is needed.

## Goal

The few steps no API or pipeline should do are as short and safe as possible: typing a secret value, and the Google console steps (OAuth consent screen and client, billing link, domain).

## Files

| Action | Path | What |
| --- | --- | --- |
| Create | `scripts/infra/set-secret.sh` | silent prompt, then `gcloud secrets versions add` |
| Create | `scripts/infra/tests/set_secret_test.sh` | stub-based tests |
| Create | `docs/runbooks/console-steps.md` | exact clicks and what to send back, per environment |

## Algorithm

1. `set-secret.sh --project <id> --name <secret> [--account <email>]`: refuses unless the container exists (Terraform creates it; never create it here), reads the value with `read -rs` (no echo, never an argument, never a file, never logged), pipes it to `gcloud secrets versions add <name> --data-file=-`, unsets the variable, prints only the version number. Names are restricted to the four in the README.
2. The runbook lists, for staging and production separately: APIs to confirm (Gmail, Drive), consent screen settings and the five scopes, OAuth client creation with the exact redirect URIs (`{origin}/api/v1/auth/google/callback`), where the client ID goes (an Environment variable, it is public) and where the secret goes (`set-secret.sh`), billing link, and domain registration. State clearly that production needs its own consent screen and client.

## Acceptance criteria

| ID | Behaviour (one line) |
| --- | --- |
| OWN AC1 | The value never appears in arguments, output, logs or files |
| OWN AC2 | It refuses a secret name that is not on the allowed list or whose container does not exist |

## Tests that must pass

- `own_ac1_value_never_in_argv_or_output` (the `gcloud` stub records argv and stdin; the test asserts the value appears only on stdin), `own_ac2_refuses_unknown_or_missing_secret`, `shellcheck` clean.

## Edge cases and traps

- Do not accept the value as a positional argument or environment variable.
- Do not echo the Client ID's neighbour, the secret, in any error message.

## Out of scope

- Creating secret containers (Terraform); pipeline and bootstrap (T-1116, T-1117).

## Done when

- Tests above pass and every required check is green (S10 10.1); Definition of done in S10 10.4.
