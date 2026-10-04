# T-205b: fake-google, part 2: Drive appDataFolder

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M2 | sonnet | about 300 lines of code plus tests | T-205a |

Split from T-205 (index row "fake-google: Gmail and Drive HTTP fake").

**Read only these spec sections:** S10 4.2 "App folder" bullet (`docs/specs/S10-test-strategy.md`); `docs/backlog/T-405-drive-app-folder-store.md` "Per-call contract (S8 for Drive)" table and the paragraph under it (the exact calls this fake answers); `docs/backlog/T-203-fake-mailbox-and-contract-suite.md` (`AppFolderControl`); `docs/backlog/T-205a-fake-google-gmail.md` (server structure, tokens, failure rules, error body). Nothing else is needed.

## Goal

`fake-google` also answers the six Drive calls the `DriveAppFolder` adapter (T-405) makes, scoped to the hidden `appDataFolder` and with a v2 `etag` that changes on every write and is enforced by `If-Match` (412 on mismatch). Tests can simulate "the user deleted the file" and read back what was stored.

## Files

| Action | Path | What |
| --- | --- | --- |
| Create | `backend/crates/fake-google/src/drive.rs` | Drive routes and state |
| Change | `backend/crates/fake-google/src/state.rs` | per-mailbox `DriveSpace` |
| Change | `backend/crates/fake-google/src/control.rs` | Drive control routes |
| Change | `backend/crates/fake-google/src/lib.rs` | mount Drive routes; handle methods below |
| Create | `backend/crates/fake-google/src/app_folder_control.rs` | `impl AppFolderControl` |
| Create | `backend/crates/fake-google/tests/drive_routes.rs` | route tests |

## Types and signatures

```rust
pub struct DriveFile { pub id: String, pub name: String, pub parents: Vec<String>, pub bytes: Vec<u8>,
                       pub version: u64, pub modified_time: OffsetDateTime, pub trashed: bool }
pub struct DriveSpace { pub files: BTreeMap<String, DriveFile>, pub next_id: u64 }

impl FakeGoogleHandle {
    pub fn drive_files(&self, mb: &FakeMailboxKey) -> Vec<(String, String, usize)>; // (id, name, len); no contents
    pub fn drive_file_bytes(&self, mb: &FakeMailboxKey, id: &str) -> Option<Vec<u8>>;
    pub fn user_deletes_app_files(&self, mb: &FakeMailboxKey);
    pub fn app_folder_control(&self, mb: &FakeMailboxKey) -> Arc<dyn testkit::contract::app_folder_store::AppFolderControl>;
}
```

Routes (every route needs a token with `drive.appdata`, else 403 `insufficientPermissions`; a token with only Gmail scopes is refused, which T-701's "unsub has no Drive" tests rely on):

| Method and path | Behaviour |
| --- | --- |
| `GET /drive/v3/files` | requires `spaces=appDataFolder` (anything else 403 `insufficientScopes`); supports `q` of the form `name = '<n>' and trashed = false` only (other `q` gives 400); `pageSize`; returns `{"files":[{"id","name","modifiedTime"}]}` |
| `GET /drive/v2/files/{id}` | `{"id","etag","title","modifiedDate"}`; 404 if absent |
| `GET /drive/v3/files/{id}?alt=media` | raw bytes, `Content-Type: application/octet-stream`; without `alt=media` returns v3 metadata JSON |
| `POST /upload/drive/v3/files?uploadType=multipart` | body `multipart/related` (boundary from `Content-Type`): first part JSON `{"name","parents":["appDataFolder"]}`, second part the bytes. `parents` must be exactly `["appDataFolder"]`, else 403. Returns `{"id"}` |
| `PUT /upload/drive/v2/files/{id}?uploadType=media` | header `If-Match` required for this fake `[DEFAULT]`; equal to current etag: replace bytes, bump version, return `{"id","etag"}`; different: 412 `conditionNotMet`; missing header: 428 `preconditionRequired`; file absent: 404 |
| `DELETE /drive/v3/files/{id}` | 204; 404 if absent |

Control routes: `POST /__fake/drive/user-deletes {email}`; `GET /__fake/drive/files/{email}` (ids, names, sizes).

## Algorithm

1. Each mailbox has its own `DriveSpace`. File IDs are `"appdata-" + counter` (opaque to the adapter).
2. `etag` = `"\"v<version>\""`: the quotes are part of the value, like Drive's. Create sets version 1. Each successful update increments it.
3. `If-Match` compares the header value byte for byte with the current etag (quotes included).
4. Multipart parsing: split on `--<boundary>`; each part has headers, a blank line and content; take the first part as JSON and the second as bytes. Bodies over 5 MiB give 413.
5. `user_deletes_app_files` removes every file in that mailbox's space (S10 4.2 scenario).
6. Failure rules from T-205a apply to Drive routes unchanged (method plus path prefix).
7. Record a `Request` event per call, with route templates `drive.files.list`, `drive.files.get.v2`, `drive.files.get.media`, `drive.files.create`, `drive.files.update.v2`, `drive.files.delete`, and the query pairs (T-405 asserts `spaces=appDataFolder`).

## Acceptance criteria

| ID | Behaviour (one line) |
| --- | --- |
| None | Fake only. It lets T-405 prove the app folder write precondition and T-701 prove `unsub` has no Drive access |

## Tests that must pass

- `fake_drive_create_list_get_media_round_trip` (integration).
- `fake_drive_update_with_current_etag_changes_etag` (integration).
- `fake_drive_update_with_stale_etag_is_412` (integration).
- `fake_drive_update_without_if_match_is_428` (integration).
- `fake_drive_list_without_appdata_space_is_refused` and `fake_drive_create_outside_appdata_is_403` (integration).
- `fake_drive_gmail_only_token_is_403` (integration).
- `fake_drive_user_deletes_file_then_list_is_empty` (integration).
- (T-405, not this task, runs `testkit::contract::app_folder_store` against this fake.)

## Edge cases and traps

- Keep the quotes in the etag; T-405 sends it back unchanged.
- Drive files are not messages: `DELETE /drive/v3/files/{id}` is allowed and is not a `PERMANENT_DELETE_ATTEMPTED` event (INV-5 is about messages only).
- Do not support any other space or parent; a fake that accepts `parents: ["root"]` would hide a scope bug.
- Never log or record file contents in events; record the size only.
- Multipart boundaries may be quoted in `Content-Type`; strip the quotes.

## Out of scope

- The adapter itself and its contract run (T-405). OAuth token issuance (T-206).

## Done when

- The tests above pass and every required check is green (S10 10.1).
- Definition of done in S10 10.4.
- The route table above is copied into `drive.rs` as a module doc comment.
