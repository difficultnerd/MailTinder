# T-1104: Build and deploy pipeline

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M11 | sonnet | about 300 lines (Dockerfile, workflow, tests) | T-006, T-1103, needs S11 |

**Read only these spec sections:** S4 sections 1 (Container images, Infrastructure as code, Environments) and 4 (Operational excellence row) (`docs/specs/S4-architecture.md`); register rows V13.4.1 and V13.4.2 in `docs/security/asvs-l2-register.md`; T-1102b "Types and signatures" (outputs used here). Nothing else is needed.

## Goal

Containers for `api`, `unsub` and `worker` that hold only the release binary, a GitHub Actions workflow that builds them once per commit on `main`, deploys them and the Flutter web app to staging, runs the staging smoke tests (T-1105), and promotes the same image digests to production after James approves.

## Files

| Action | Path | What |
| --- | --- | --- |
| Create | `backend/Dockerfile` | Multi-stage, `ARG SERVICE`, final image distroless with one binary |
| Create | `backend/.dockerignore` | Exclude `target/`, tests, fixtures |
| Create | `.github/workflows/deploy.yml` | Build, deploy staging, smoke, promote to production |
| Create | `.firebaserc` | Aliases `staging` and `prod` with placeholder project IDs filled from repository variables at deploy time |
| Create | `backend/crates/api/tests/dockerfile_policy.rs` | Reads `backend/Dockerfile` as text |
| Change | `infra/terraform/README.md` | GitHub variables and environments James sets |

## Types and signatures

```dockerfile
# backend/Dockerfile
ARG SERVICE                      # api | unsub | worker
FROM rust:<pinned stable>-bookworm AS build
WORKDIR /src
COPY . .
RUN cargo build --release --locked -p ${SERVICE}
FROM gcr.io/distroless/cc-debian12:nonroot
COPY --from=build /src/target/release/${SERVICE} /app
USER nonroot
ENTRYPOINT ["/app"]
```

```yaml
# deploy.yml jobs (names are fixed; T-1105 calls into `smoke`)
# build:        matrix [api, unsub, worker]; push to staging Artifact Registry tagged ${{ github.sha }}; output digests
# deploy-staging: environment: staging (its WIF provider pins assertion.environment == 'staging'); gcloud run deploy <svc> --image <repo>/<svc>@<digest> --region us-central1 (image only); firebase deploy --only hosting
# smoke:        reusable workflow from T-1105
# deploy-prod:  environment: production (James approves); copy digests to the prod repository; deploy the same digests; firebase deploy
```

## Algorithm

1. **Triggers:** `push` to `main` and `workflow_dispatch`. `concurrency: deploy` with `cancel-in-progress: false`. `permissions: contents: read, id-token: write`.
2. **Auth:** `google-github-actions/auth` with `workload_identity_provider` and `service_account` from repository variables (`STAGING_WIF_PROVIDER`, `STAGING_DEPLOYER`, `PROD_WIF_PROVIDER`, `PROD_DEPLOYER`), outputs of T-1102b. No JSON keys.
3. **Build** each service with `docker build --build-arg SERVICE=<svc> -f backend/Dockerfile backend`, push to `us-central1-docker.pkg.dev/<staging-project>/mailtinder/<svc>:<sha>`, record the digest.
4. **Web:** `flutter build web --release` (no `MT_E2E` define), then `firebase deploy --only hosting --project staging` using `firebase-tools` pinned to an exact version through `npx` (the app's headers and rewrites come from `firebase.json`, owned by T-006).
5. **Deploy staging:** job with `environment: staging` (a GitHub environment with no required reviewer). The staging WIF provider (T-1103 sets `deploy_environment = "staging"`) pins `assertion.environment == 'staging'`, so a job that names no environment presents no claim and its token is rejected. `gcloud run deploy <svc> --image ...@<digest> --region us-central1 --quiet` with no other flags, so Terraform keeps owning env vars, service accounts and scaling.
6. **Smoke:** call T-1105's reusable workflow; production is blocked unless it passes.
7. **Promote to production:** job with `environment: production` (GitHub environment with James as required reviewer `[DEFAULT]`). Copy each digest from the staging repository to the production repository with `gcloud artifacts docker images copy` or `crane copy` (pin the tool), deploy those exact digests, then deploy Hosting to `prod` from the same commit.
8. **Rollback** `[DEFAULT]`: documented in the README as `gcloud run services update-traffic <svc> --to-revisions=<previous>=100`; no automation.
9. **Policy test** (`dockerfile_policy.rs`): reads `../../Dockerfile` and asserts the final stage is a distroless `nonroot` image, copies exactly one file from the build stage, sets `USER nonroot`, and the build command has `--release --locked` and never `--features` or `testkit`.

**Waits on S11** (defaults used): release cadence and who approves production (`[DEFAULT]` every merge to `main` reaches staging; James approves production); rollback runbook; whether staging deploys on every merge or on a schedule; image retention in Artifact Registry (`[DEFAULT]` keep all); who reads Artifact Registry scanning findings if it is ever turned on.

## Acceptance criteria

| ID | Behaviour (one line) |
| --- | --- |
| ASVS V13.4.1 | The container holds only the release binary |
| ASVS V13.4.2 | The release build has no test or debug features |

## Tests that must pass

- `asvs_v13_4_1_dockerfile_final_stage_holds_only_the_binary` (unit, `api` tests, reads the Dockerfile)
- `asvs_v13_4_2_dockerfile_builds_release_without_features` (unit, same file)
- A `workflow_dispatch` run of `deploy.yml` deploys staging and passes the smoke job (link in the PR).

## Edge cases and traps

- Build once, promote by digest: production must run the bytes staging tested; never rebuild for production.
- `gcloud run deploy` with `--set-env-vars`, `--service-account` or scaling flags would fight Terraform; pass `--image` only.
- Pin every action by full commit SHA like the existing workflows, and pin `firebase-tools` and the Rust image tag.
- The deploy job must not run on pull requests (no secrets or WIF tokens for forks).
- Each deploy job must name its GitHub environment (`environment: staging` for the staging job, `environment: production` for the promote job): each WIF provider's condition pins `assertion.environment`, so a job that names no environment is rejected server-side (T-1102b, T-1103).
- Never print tokens; `gcloud` and `firebase` read credentials from the auth step.
- No container scanning step in CI (CLAUDE.md); Artifact Registry scanning, if ever enabled, runs in Google Cloud.

## Out of scope

- Smoke tests and ZAP (T-1105); Terraform changes (T-1102a, T-1102b, T-1103).

## Done when

- The tests above pass and every required check is green (S10 10.1).
- Definition of done in S10 10.4.
- James has created the `production` GitHub environment with himself as required reviewer, a `staging` GitHub environment with no required reviewer, and set the four WIF variables.
