# ADR 0003: infrastructure changes: the owner decides, a pipeline applies

Status: proposed (2026-10-10, written at the owner's request: "a deployment method where I'm a decider, not a meat proxy"; needs the owner to accept)
Date: 10 October 2026

## Context

Today the Terraform README makes the owner the operator: create the project, create the state bucket, run `terraform init`, `plan`, `apply` from his own machine, then add secret values. Two environments (staging `mailtinderstaging` and the production project) both exist as Google Cloud projects and production has no configuration yet. The owner wants to review and approve, not to type or click.

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

1. **Plan on every infrastructure PR, automatically, read-only.** A `planner` identity (narrow view-only roles, state read-only, `terraform plan -lock=false`) produces a plan and posts a **redacted summary** on the PR: counts, and for each change the resource address, type and action, never values, with changed attribute names and a `SENSITIVE` flag on identity, logging, retention and key changes. The PR plan runs the summary tool from `main` and does not authenticate when the PR changes the workflow, the tool or `scripts/infra/**`. The owner reads a summary, not commands.
2. **Apply only from `main`, in a GitHub Environment, after a read-only plan has been shown.** On every push to `main` a plan job (planner identity, no approval needed because it is read-only) publishes the redacted summary and its digest. The apply job runs in the Environment (`staging` after merge; **`production` waits for the owner's approval**), **re-plans with the applier identity, recomputes the summary digest and aborts if it differs from the approved one**, then applies that plan in the same job. Terraform additionally refuses a plan that is stale against current state. We deliberately do NOT upload the plan file as an artifact: in a public repository artifacts are downloadable by any signed-in GitHub user and a plan file contains full values.
3. **No keys, and the privileged identity is unreachable except from the right job.** Planner and applier live in **two separate Workload Identity pools** (`github-plan`, `github-apply`). The applier provider's condition pins repository, `refs/heads/main`, the environment name AND `job_workflow_ref` (`<repo>/.github/workflows/terraform.yml@refs/heads/main`); the applier's impersonation binding is on `attribute.environment` and `attribute.ref` that only the apply provider maps. The planner provider admits same-repository pull-request runs and the main-branch plan job, nothing else. A planner-pool token can never impersonate the applier (a bootstrap test proves it).
4. **The applier cannot grant itself ownership.** Its project-IAM-admin grant carries an IAM Condition limiting the roles it may grant to the table of roles the modules bind (T-1116), and every other role is resource-scoped.
5. **Protected resources cannot be destroyed by this pipeline.** The workflow checks every plan against a deny-list of addresses (log bucket, KMS key rings and keys, Firestore database, Secret Manager secrets, the state bucket, the Artifact Registry repository, the identity pools and the planner and applier service accounts) and fails on any delete or replace before the approval prompt; this does not rely on `prevent_destroy` (which disappears if someone deletes the block).
6. **One bootstrap per project, by the owner, one command** (T-1116): state bucket, first APIs, the two pools and service accounts.
7. **Preconditions the owner controls.** Apply jobs run only when the repository variable `TF_APPLY_ENABLED` is `true`. The owner sets it after enabling "Require review from Code Owners" on `main` (as of 10 Oct 2026 `main` has no required pull-request review, checked via the API) with `CODEOWNERS` covering `.github/workflows/**`, `infra/**` and `scripts/infra/**`; until then plans run but nothing applies.
8. **What stays human by nature** (no API, or must not be automated): the OAuth consent screen and client, the billing link, the domain, and typing secret values (T-1118).

## Consequences

- Two identities per project appear in separate pools (planner: narrow view-only; applier: broad but conditioned). The applier is the most powerful thing in the system; its attribute condition and the environment approval are the controls and are reviewed accordingly (strong tier, cross-vendor review).
- The workflow file (`.github/workflows/terraform.yml`) is owner-merged, like `ci.yml`.
- Destructive or irreversible changes (locked log bucket, KMS keys, Firestore location) already carry `prevent_destroy` or documented warnings; the plan summary highlights any destroy or replace so the approver sees it.
- The approval is on a redacted summary with a digest the apply job re-checks; it is not a signature over a plan file. Say so plainly to the approver.
- Tasks: T-1116 (bootstrap), T-1117 (plan/apply workflow), T-1118 (owner-input helpers and console checklist).
- Revisit if we productise the factory for other tenants (architecture options s.16): the same pattern per tenant project.
