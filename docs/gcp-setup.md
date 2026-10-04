# GCP setup (MailTinder staging)

Reference for the build agent and for anyone restoring this environment. This
documents what is configured in Google Cloud, how the VPS authenticates, and
what is still outstanding.

## Project

- **Name:** MailTinderStaging
- **Project ID:** `mailtinderstaging`
- **Project number:** `432370989007`
- **Owner:** `jnewburrie@gmail.com`
- **Billing:** attached; user has $300 Google credit over 90 days. **Stay in
  the free tier** — every service below is free-tier eligible.

## Enabled APIs

```
firestore.googleapis.com
run.googleapis.com
cloudtasks.googleapis.com
secretmanager.googleapis.com
cloudkms.googleapis.com
aiplatform.googleapis.com
gmail.googleapis.com
cloudresourcemanager.googleapis.com
```

## Firestore

- Database created in **native mode**, location **us-central1** (matches the
  `region` variable in the architecture spec).

## Build service account

- **Email:** `mailtinder-agent@mailtinderstaging.iam.gserviceaccount.com`
- **Key file (VPS):** `/home/ubuntu/.gcp-agent-key.json`
- **ADC:** `GOOGLE_APPLICATION_CREDENTIALS=/home/ubuntu/.gcp-agent-key.json`
  (exported in `~/.bashrc`, so it persists across sessions).
- **Roles granted (project-level):**
  - `roles/datastore.user` (Firestore read/write)
  - `roles/secretmanager.secretAccessor`
  - `roles/cloudtasks.enqueuer`
  - `roles/aiplatform.user`
  - `roles/run.viewer`
- **Not yet granted:** `roles/cloudkms.cryptoEncrypterDecrypter` — this is a
  resource-level role, grant it on the KMS key itself once the key exists
  (Terraform will do this).

## gcloud auth on the VPS

Two accounts are available:
- `jnewburrie@gmail.com` — the **admin** account (owner). Use this for IAM
  changes, creating resources, granting roles.
- `mailtinder-agent@...` — the **service account** (ADC). Use this for the
  agent's runtime code.

Switch with `gcloud config set account <email>`. The service account cannot
grant its own roles or create project-level resources — switch to the admin
account for those.

## How the agent uses this

- Rust adapters authenticate via **Application Default Credentials** (the
  `GOOGLE_APPLICATION_CREDENTIALS` env var → the service-account key). No
  per-call credentials needed.
- `gcloud` CLI is on the VPS at `/home/ubuntu/google-cloud-sdk/bin/gcloud`
  (symlinked to `/usr/local/bin/gcloud`).

## Outstanding (Phase B — Gmail/OAuth)

Needed for the Gmail adapter and user sign-in. Requires a human (consent
screen verification + a test Gmail account):

1. **OAuth consent screen** (APIs & Services → OAuth consent screen →
   External) — app name, owner email, add a **test user**.
2. **OAuth client** (Credentials → Create Credentials → OAuth client ID →
   Web application) — redirect URI `http://localhost:8080/oauth2callback`
   (plus the Cloud Run URL later). Note the Client ID + Secret.
3. **Test Gmail account** — a throwaway Gmail address, added as a test user.
4. Store the Client ID + Secret in **Secret Manager** (the `api`/`unsub`
   service accounts are the accessors) and tell the agent the secret names.

## Restore / re-provision

If the VPS is rebuilt, re-run:
```bash
gcloud auth login --no-launch-browser   # as jnewburrie@gmail.com
gcloud config set project mailtinderstaging
# re-create the service-account key if lost:
gcloud iam service-accounts keys create /home/ubuntu/.gcp-agent-key.json \
  --iam-account mailtinder-agent@mailtinderstaging.iam.gserviceaccount.com
echo 'export GOOGLE_APPLICATION_CREDENTIALS=/home/ubuntu/.gcp-agent-key.json' >> ~/.bashrc
```
