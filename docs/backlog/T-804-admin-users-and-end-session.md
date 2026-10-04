# T-804: Admin users and ending a session

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M8 | sonnet | about 200 lines of code plus tests | T-501, T-504 |

**Read only these spec sections:** S7 sections 3.7 (roles), 5.11 (API-ADM-15, API-ADM-16) and 6 (admin session kill limit) in `docs/specs/S7-api-contract.md`; the `/admin/users` and `/admin/users/{user_id}/sessions` paths in `docs/specs/S7-api-contract.openapi.yaml`; S2 AU-01 AC5, AU-07 AC5 and AC6; S6 section 7. Nothing else is needed.

## Goal

An admin can list users (address joined with, mailbox count, whether signed in, last seen) and end one user's session after a fresh Google sign-in, with a security event.

## Files

| Action | Path | What |
| --- | --- | --- |
| Create | `backend/crates/api/src/routes/admin_users.rs` | Both handlers and DTOs |
| Change | `backend/crates/api/src/routes/mod.rs` | Mount under `/admin`; Firestore limit 20 session ends per admin per day |
| Create | `backend/crates/api/tests/admin_users.rs` | Service integration tests |

## Types and signatures

```rust
#[derive(Serialize)]
pub struct AdminUserDto { pub user_id: Uuid, pub email_address: String, pub created_at: OffsetDateTime,
    pub is_admin: bool, pub mailbox_count: u32, pub signed_in: bool, pub last_seen_at: Option<OffsetDateTime> }
#[derive(Serialize)] pub struct AdminUserPage { pub users: Vec<AdminUserDto>, pub next_cursor: Option<String> }

pub async fn list_users(app: &AppState, admin: &AuthedSession, cursor: Option<&str>, limit: u32) -> Result<AdminUserPage, ApiError>;
pub async fn end_user_session(app: &AppState, admin: &AuthedSession, user: Uuid) -> Result<(), ApiError>;
```

## Algorithm

1. Both routes: `session.is_admin` checked server side; otherwise `403 forbidden` and a security event (S7 3.7).
2. API-ADM-16: `limit` 1 to 50. `users().list(PageRequest { limit, after })`; the store cursor is sealed as `TokenType::Cursor` before it leaves (T-201b). For each user: `mailboxes().by_user`; `email_address` is the earliest linked mailbox's address, decrypted with that user's key `[DEFAULT]` (S7: "the address the user joined with"; the first mailbox may have been disconnected); `mailbox_count`; `sessions().by_user`: `signed_in` is true when an `authenticated` record has not passed its idle or absolute expiry at `Clock::now`, and `last_seen_at` is its `last_seen_at`, else `null`.
3. API-ADM-15: `require_step_up` (AU-01 AC5); rate limit; user missing: `404 not_found`; `sessions().delete_all_for_user(user)`; security event `session_ended_by_admin` with both pseudonymous IDs (AU-07 AC5, AC6); `204`. Queued jobs keep running (S7 5.11). An admin ending their own session is allowed and ends it.

## Acceptance criteria

| ID | Behaviour (one line) |
| --- | --- |
| AU-07 AC5 | An admin ends a user's session; that user's next request gets `401` |
| AU-07 AC6 | An admin ending a session is recorded as a security event |
| AU-01 AC5 | Ending a session needs step-up |
| V7.4.5 | Administrators can end an individual user's session |
| V8.2.1 | A non-admin is refused on both routes |

## Tests that must pass

- `au_07_ac5_admin_ends_user_session` (service integration)
- `au_07_ac5_jobs_keep_running_after_session_end` (service integration: queued job still `queued`)
- `au_07_ac6_admin_session_end_logged` (service integration: captured security event, no address in it)
- `au_01_ac5_end_session_needs_step_up` (service integration)
- `asvs_v7_4_5_admin_terminates_session` (service integration)
- `asvs_v8_2_1_non_admin_refused_and_logged` (service integration, both routes)
- `s9_admin_users_list_fields` (service integration: signed-in and signed-out users, mailbox counts)
- `admin_users_rate_limit_20_per_day` (service integration: 21st call `429`)

## Edge cases and traps

- The admin flag is read from the user record, never from the request.
- `user_id` is the only path parameter; never accept an address in the URL.
- Decrypt each user's address with that user's own key; the admin's key opens nothing of theirs.
- The response holds addresses; no logging of them and `no-store` applies.

## Out of scope

- Ending all users' sessions: operations runbook (S11).

## Done when

- The tests above pass and every required check is green (S10 10.1).
- Definition of done in S10 10.4.
