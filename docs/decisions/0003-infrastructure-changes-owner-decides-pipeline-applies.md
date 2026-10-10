# ADR 0003: infrastructure changes: the owner decides, a pipeline applies

Status: proposed (2026-10-10, written at the owner's request: "a deployment method where I'm a decider, not a meat proxy"; needs the owner to accept)
Date: 10 October 2026

## Context

Today the Terraform README makes the owner the operator: create the project, create the state bucket, run `terraform init`, `plan`, `apply` from his own machine, then add secret values. Two environments (staging `mailtinderstaging`, production `MailTinderProd`) both exist as Google Cloud projects and production has no configuration yet. The owner wants to review and approve, not to type or click.

Facts checked on 2026-10-10 (from the PR #18/#20 branches, which this ADR's tasks depend on, and the repository settings):
- `infra/terraform/modules/runtime/deploy_identity.tf` (lines 5 to 45) already builds a keyless GitHub identity: a Workload Identity pool and provider whose `attribute_condition` pins repository, `ref == refs/heads/main` and a GitHub **environment**, plus a service account only that pool can impersonate. The README rule is "No service account keys, ever".
- The repository already has GitHub Environments: `production` (required reviewers + branch policy) and `staging` (branch policy only).
- The `terraform` job in `.github/workflows/ci.yml` (line 94 on the #20 branch) runs `fmt`, `validate` and `test` with `-backend=false`, no cloud credentials.
- S4 1: "Agents write it; James reviews the plan before each apply."

## Options (buy / borrow / build)

| Option | Verdict |
| --- | --- |
| A. Keep it manual (owner runs apply from his machine) | Rejected: it is the problem being solved |
| B. Hosted Terraform service (HCP Terraform, Spacelift) | Rejected for now: holds our state and cloud credentials in a third party, adds cost and a vendor, and does not use the approval gate we already have |
| C. Self-hosted Atlantis | Rejected: a server to run, patch and secure |
| D. Give an agent a key or owner credentials | Rejected: violates "no keys", puts a high-power credential next to agents that read untrusted text |
| **E. GitHub Actions + Environments + Workload Identity Federation** | **Chosen. Borrow:** `google-github-actions/auth`, `hashicorp/setup-terraform` and GitHub's environment approval. **Build:** a one-time bootstrap script, one workflow file, small helpers |

## Decision

1. **Plan on every infrastructure PR, automatically, read-only.** A `planner` identity (view-only, state read) produces the plan and posts a summary on the PR. The owner reads a plan, not commands.
2. **Apply only from `main`, in a GitHub Environment, as the exact plan that was shown.** The plan job saves the plan file and prints its checksum; the apply job downloads that same file and runs `terraform apply <planfile>` (Terraform itself refuses a stale plan). `staging` applies after merge; **`production` waits for the owner's approval** (one tap on the environment's required-reviewer prompt). A production apply needs a green staging apply first when both changed.
3. **No keys.** The `applier` identity is a separate Workload Identity pool and service account per project whose condition pins repository, `refs/heads/main` and the environment name, exactly like the existing deployer. It can only be reached by the apply job after the approval gate.
4. **One bootstrap per project, by the owner, one command.** The only things Terraform cannot do for itself are creating the state bucket, enabling the first APIs and creating the applier identity. `scripts/infra/bootstrap-project.sh` does all three after showing what it will do (T-1116).
5. **What stays human by nature** (no API, or must not be automated): the OAuth consent screen and OAuth client, the billing link, the domain, and typing secret values. These get exact, short helpers and a checklist (T-1118).

## Consequences

- Two privileged identities per project appear (planner: view-only; applier: broad). The applier is the most powerful thing in the system; its attribute condition and the environment approval are the controls and are reviewed accordingly (strong tier, cross-vendor review).
- The workflow file (`.github/workflows/terraform.yml`) is owner-merged, like `ci.yml`.
- Destructive or irreversible changes (locked log bucket, KMS keys, Firestore location) already carry `prevent_destroy` or documented warnings; the plan summary highlights any destroy or replace so the approver sees it.
- Tasks: T-1116 (bootstrap), T-1117 (plan/apply workflow), T-1118 (owner-input helpers and console checklist).
- Revisit if we productise the factory for other tenants (architecture options s.16): the same pattern per tenant project.
