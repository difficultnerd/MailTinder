# Runbook: the console steps (staging and production)

The OAuth consent screen and client, the billing link and the domain are the
steps no API or pipeline should do (ADR 0003 decision 8), and they are the
owner's. Everything here is done by James, signed in as the project owner, with
the project open in the Cloud console. Terraform does everything else; nothing
in this runbook creates a container, an API grant or a role.

Do it once per environment, in this order. Staging first, so the flow is known
before production gets its own.

## Rules that apply to both environments

- **The client ID is public. The client secret is not.** The client ID goes
  into `terraform.tfvars` (never committed), which Terraform passes to `api`
  and `unsub` as the environment variable `GOOGLE_OAUTH_CLIENT_ID`. Send the
  client ID back, paste it anywhere: it is a public identifier.
- **The client secret is typed, not sent.** It goes in through
  `scripts/infra/set-secret.sh`, which reads it hidden and pipes it to
  `gcloud secrets versions add`. Do not paste it into a chat, a ticket, a
  shell command, or a file; do not put it in `terraform.tfvars`. If a secret
  value ever appears outside that prompt, rotate it (reset the OAuth client
  secret, or mint fresh HMAC keys with `openssl rand -base64 32`).
- **Never create the secret container by hand.** Terraform creates all four
  (`infra/terraform/README.md` step 5); `set-secret.sh` refuses a name that is
  not on that list, and refuses one whose container does not exist yet.
- **The redirect URI must match exactly**: the scheme, the host and the path,
  with no trailing slash and no wildcard. Google refuses the sign-in with
  `redirect_uri_mismatch` otherwise. Every environment is its own project with
  its own origin.

The callback path is always:

```
{origin}/api/v1/auth/google/callback
```

`{origin}` is that environment's `APP_ORIGIN`: `https`, the exact host the
Flutter app is served from, no trailing slash.

## Staging (`mailtinderstaging`)

Staging holds synthetic data only and stays in Testing mode (S11 2).

1. **Confirm the APIs.** APIs & Services → Enabled APIs & services: Gmail API
   (`gmail.googleapis.com`) and Google Drive API (`drive.googleapis.com`) are
   listed as enabled. Terraform enables them; this step confirms it. If either
   is missing, re-apply the infrastructure module rather than enabling it by
   hand, so the configuration and the project agree.

2. **Consent screen.** APIs & Services → OAuth consent screen. User type
   **External**; app name `Mail Tinder (staging)`; user support email and
   developer contact email: your own. Authorised domains: the staging Hosting
   host (`<staging-site>.web.app`). Save.

3. **The five scopes.** On the same screen, add exactly these — no others:

   ```
   openid
   email
   https://www.googleapis.com/auth/gmail.modify
   https://www.googleapis.com/auth/gmail.send
   https://www.googleapis.com/auth/drive.appdata
   ```

   They are the closed list in `backend/crates/adapters-gmail/src/scopes.rs`
   (S6 4). Adding a scope here that the code never asks for, or asking for one
   the screen does not list, both fail the sign-in.

4. **Publishing status: Testing**, and add your own Gmail address plus any
   test account as **test users**. Nobody else can sign in while it is in
   Testing, which is what staging wants.

5. **OAuth client.** Credentials → Create credentials → OAuth client ID →
   **Web application**. Authorised redirect URIs — exactly one:

   ```
   https://<staging-site>.web.app/api/v1/auth/google/callback
   ```

   (the staging Hosting URL, the `STAGING_URL` repository variable). Copy the
   client ID.

6. **Client ID into the environment.** Put it in
   `infra/terraform/envs/staging/terraform.tfvars` (never committed) as
   `google_oauth_client_id`. Terraform passes it to `api` and `unsub` as
   `GOOGLE_OAUTH_CLIENT_ID`; a blank value fails the plan. Send the client ID
   to the build thread.

7. **Client secret into Secret Manager.** From the repository root:

   ```
   scripts/infra/set-secret.sh --project mailtinderstaging \
     --name google-oauth-client-secret
   ```

   Add `--account <the owner account>` first if `gcloud`'s default account is
   not the owner of that project. It prints the new version number; report that
   number (never the value).

8. **Billing.** Console → Billing → link the staging project to the billing
   account (`infra/terraform/README.md` step 1). The two budgets and the email
   channel come from Terraform (T-1107); nothing else is clicked here.

9. **The HMAC keys.** Not from the console: generate them with
   `openssl rand -base64 32` and type each through `set-secret.sh` as
   `email-lookup-hmac-key` and `log-pseudonym-hmac-key`. Fresh keys for
   staging, never a copy of production's.

## Production

Production repeats every step above against the production project, with three
differences that matter:

1. **Its own consent screen and its own OAuth client.** Not a reuse of
   staging's, and not a second redirect URI on staging's client: staging
   isolation is a property the trial relies on (S11 2). Its own project, its
   own client, its own secret container value.
2. **Its own redirect URI**, on the production origin:

   ```
   https://mailtinder.app/api/v1/auth/google/callback
   ```

   (or the production Hosting URL, `https://<prod-site>.web.app`, until the
   custom domain is connected - whichever the hosted app is actually served
   from, and then `APP_ORIGIN` and this URI must be changed together).
3. **Publishing status.** `gmail.modify` is a restricted scope, so public
   publication means Google verification, which the plan defers to v2
   (`docs/plan-open-questions.md` B1): v1 stays in **Testing** with the owner's
   own accounts as test users. Staging's own consent screen stays in Testing
   regardless.

## The domain (production only)

Staging keeps its `<staging-site>.web.app` URL: it needs no registration and
keeps the redirect URI short. Production gets the real one.

1. If the domain is not registered yet, register it at the registrar of your
   choice. The code's default origin is `https://mailtinder.app`
   (`backend/crates/admin-cli`), so use that name unless you decide otherwise
   — and then change the default origin as well.
2. Firebase console → the production project → Hosting → **Add custom
   domain** → enter the domain. Firebase prints the DNS records (usually one
   `A`-record pair or a `TXT` plus a `CNAME`).
3. Add those records at the registrar. The certificate is issued once they
   resolve; the domain shows `Connected` when it is done. This can take an
   hour or a day, and nothing else here waits on it.
4. Then set `APP_ORIGIN` to the custom domain and make sure the client's
   redirect URI above matches it. Until then, production runs on its
   `web.app` URL and should say so.

## What to send back

For each environment, one message, no secrets in it:

- the client ID (public, so it can go in `terraform.tfvars`);
- the origin the app is served from (so `APP_ORIGIN` and the redirect URI
  agree);
- the version numbers `set-secret.sh` printed for the OAuth client secret and
  the two HMAC keys;
- confirmation that billing is linked, and for production that the domain is
  `Connected` (or that it is still pending, and which URL production is on).

Never send: the client secret, the HMAC keys, or any value you typed at the
prompt. If one of those ends up somewhere written down, rotate it and say so.
