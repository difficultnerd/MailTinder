# T-507: Admin bootstrap command line tool

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M5 | sonnet | about 250 lines of code plus tests | T-301, T-302, T-305, T-307, T-505 |

**Read only these spec sections:** S7 section 3.7 "Roles" (`docs/specs/S7-api-contract.md`); S6 section 5 rows "System KMS key" and "Email lookup HMAC key" (`docs/specs/S6-security.md`); ASVS register row V6.3.2 (`docs/security/asvs-l2-register.md`); `docs/backlog/T-505-invites-and-invite-requests.md` "`issue_invite`" steps 1 to 5. Nothing else is needed.

## Goal

The app has no admin until one is made, and nobody can sign in without an invite. James runs a small Rust tool from his own machine, with his own Google credentials, to (1) create the first invite and print its link, and (2) after he signs in, mark his user as admin. The API never grants the admin flag (S7 3.7, James, 4 October 2026).

## Files

| Action | Path | What |
| --- | --- | --- |
| Create | `backend/crates/admin-cli/Cargo.toml` | New binary crate `mt-admin`; depends on `domain`, `ports`, `adapters-gcp`, `obs`, `svc-common`, `clap` (derive); never on `api`; add to workspace members |
| Create | `backend/crates/admin-cli/src/main.rs` | Argument parsing, adapter wiring, exit codes |
| Create | `backend/crates/admin-cli/src/commands.rs` | `invite`, `list_users`, `make_admin` over the port traits |
| Create | `backend/crates/svc-common/src/invites.rs` | `upsert_pending_invite`: steps 1 to 5 of T-505 `issue_invite`, moved here unchanged |
| Change | `backend/crates/svc-common/src/lib.rs` | `pub mod invites;` |
| Change | `backend/crates/api/src/routes/invites.rs` | `issue_invite` calls `upsert_pending_invite`; behaviour and tests unchanged |
| Change | `backend/Cargo.toml` | `crates/admin-cli` in `members` |
| Create | `backend/crates/admin-cli/tests/commands.rs` | Tests with `testkit` fakes |
| Create | `docs/runbooks/first-admin.md` | The three steps for James |

## Types and signatures

```rust
// commands.rs: everything takes ports, so tests use testkit fakes
pub struct Deps<'a> { pub store: &'a dyn ServerStore, pub system_keys: &'a dyn SystemKeyService,
                      pub email_lookup_key: &'a Sensitive<Vec<u8>>, pub clock: &'a dyn Clock, pub rng: &'a dyn Rng,
                      pub app_origin: &'a Url }

/// Create or refresh the pending invite for `email`; returns the invite link. Sends no email.
pub async fn invite(d: &Deps<'_>, email: &EmailAddress) -> Result<Url, CliError>;
/// One line per user: user_id, created_at, is_admin, mailbox_count. No email addresses.
pub async fn list_users(d: &Deps<'_>) -> Result<Vec<UserLine>, CliError>;
/// Set is_admin = true on that user. Refuses if the user does not exist.
pub async fn make_admin(d: &Deps<'_>, user: &UserId) -> Result<MakeAdminResult, CliError>;
pub enum MakeAdminResult { Granted, AlreadyAdmin }

// svc-common/src/invites.rs
pub enum Upserted { Created, Resent }
/// Returns the stored invite, the raw token (caller builds the link, then drops it) and which branch ran.
pub async fn upsert_pending_invite(store: &dyn ServerStore, system_keys: &dyn SystemKeyService, email_lookup_key: &Sensitive<Vec<u8>>,
    clock: &dyn Clock, rng: &dyn Rng, email: &EmailAddress) -> Result<(InviteRecord, Sensitive<String>, Upserted), UpsertError>;

#[derive(Debug, thiserror::Error)]
pub enum CliError { #[error("not found")] NotFound, #[error("store: {0}")] Store(#[from] StoreError),
                    #[error("key: {0}")] Key(#[from] KeyError), #[error("bad input")] BadInput }
```

Command line: `mt-admin --project <id> invite <email>`, `mt-admin --project <id> list-users`, `mt-admin --project <id> make-admin <user_id>`.

## Algorithm

1. `main`: parse with `clap`. Build the production adapters for `--project` with Application Default Credentials (`gcloud auth application-default login`): Firestore store (T-301), `KmsSystemKeyService` (T-302), the email lookup key from Secret Manager (T-305), system clock and rng. `app_origin` comes from `--origin`, default `https://mailtinder.app` `[DEFAULT]`.
2. `invite`: parse the address with `EmailAddress::parse` (`BadInput` on failure); call the shared `upsert_pending_invite`; build the link exactly as T-505 step 6 (`{origin}/#/invite?t={raw}`); print it to stdout once; drop `raw`. Log `security_event { action: "invite_create", outcome: "created" | "resent", user: None }`.
3. `list-users`: page through `users().list` and `mailboxes().by_user` for each; print a fixed-width table to stdout.
4. `make-admin`: `users().get(user)`; `NotFound` if absent; if already admin return `AlreadyAdmin`; else set `is_admin = true` and put with `Matches(version)` (retry once on a version conflict). Log `security_event { action: "admin_action", outcome: "admin_granted", user: Some(pseudo(user)) }`.
5. Exit codes: 0 success, 2 bad input, 3 not found, 1 anything else. Errors print one line to stderr with no email address or token.

## Acceptance criteria

| ID | Behaviour (one line) |
| --- | --- |
| V6.3.2 | No default admin exists; the first admin is set only by this tool, outside the API |
| AU-01 AC1 | The bootstrap invite is a normal pending invite that T-502b redeems |

## Tests that must pass

- `asvs_v6_3_2_no_admin_until_make_admin` (integration with fakes: a fresh store has no user with `is_admin`; after `make_admin` exactly one)
- `asvs_v6_3_2_make_admin_unknown_user_not_found` (integration)
- `asvs_v6_3_2_make_admin_twice_is_already_admin` (integration)
- `au_01_ac1_cli_invite_redeemable` (integration: the printed token's SHA-256 matches the stored `token_hash`; the email is sealed with the system key and the lookup hash matches `email_lookup_hash`)
- `au_01_ac1_cli_invite_twice_resends` (integration: second call keeps one pending invite, new hash)
- `list_users_prints_no_email` (unit: output contains no `@`)

## Edge cases and traps

- Never add an API route, environment variable or start-up setting that grants admin.
- The raw invite token goes to stdout only; never to a log, a file or stderr.
- The tool needs James's account to hold Firestore, Secret Manager accessor and KMS encrypt on the system key; it must not ask for the per-user KEK.
- Do not change `issue_invite`'s responses or tests when extracting the shared function.

## Out of scope

- Revoking admin (edit by hand in the console for the trial).
- Sending the invite email (James pastes the link to himself).

## Done when

- The tests above pass and every required check is green (S10 10.1).
- `docs/runbooks/first-admin.md` lists: run `invite` with James's address; open the link and sign in; run `list-users`, then `make-admin` with the one user ID.
- Definition of done in S10 10.4.
