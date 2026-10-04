# T-405: Drive app folder store

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M4 | sonnet | about 300 lines of code plus tests | T-205b, T-302, T-306, T-401 |

**Read only these spec sections:** S3 "Where each entity lives" bullet "User app folder" (`docs/specs/S3-domain-model.md`); S5 "User app folder" section (`docs/specs/S5-data-inventory.md`); S2 AU-05 AC4, AU-06 AC1 (`docs/specs/S2-v1-acceptance-criteria.md`); S6 section 4 "Scopes (Gmail)" (`docs/specs/S6-security.md`); S10 4.2 "App folder" bullet and the "App folder writes" row of 6.3 (`docs/specs/S10-test-strategy.md`); `docs/backlog/T-401-gmail-read-messages-and-headers.md` "Error mapping"; `docs/backlog/CONVENTIONS.md` `AppFolderStore`. Nothing else is needed. This file is the S8 contract for the Drive calls.

## Goal

`adapters-gmail` gets `DriveAppFolder`, the real `AppFolderStore` for Google Drive's hidden `appDataFolder`. It reads, writes with an ETag precondition so two requests cannot overwrite each other, and deletes the one per-user file. It only ever handles ciphertext: encryption under `data_key` is the caller's job (T-302 types). The api uses it for rules, History and the Feed cursor (T-609), mailbox disconnect (T-601) and account deletion (T-803).

## Files

| Action | Path | What |
| --- | --- | --- |
| Create | `backend/crates/adapters-gmail/src/drive.rs` | `DriveAppFolder` and its wire types |
| Change | `backend/crates/adapters-gmail/src/lib.rs` | `pub mod drive; pub use drive::DriveAppFolder;` |
| Create | `backend/crates/adapters-gmail/tests/contract_app_folder.rs` | `testkit::contract::app_folder_store` run against `fake-google` |
| Create | `backend/crates/testkit/src/contract/app_folder_store.rs` | Shared contract suite (if T-203 did not already add one; it runs against the in-memory fake too) |

## Types and signatures

```rust
// drive.rs
pub const DRIVE_V3: &str = "https://www.googleapis.com/drive/v3";
pub const DRIVE_V2: &str = "https://www.googleapis.com/drive/v2";
pub const DRIVE_UPLOAD_V3: &str = "https://www.googleapis.com/upload/drive/v3";
pub const DRIVE_UPLOAD_V2: &str = "https://www.googleapis.com/upload/drive/v2";
pub const APP_FILE_NAME: &str = "mailtinder-state-v1.bin";   // [DEFAULT]
pub const APP_FILE_MAX_BYTES: usize = 5 * 1024 * 1024;        // [DEFAULT] simple upload limit; History is trimmed at 12 months (S5)

pub struct DriveAppFolder { http: GmailHttp }  // GmailHttp from T-401, constructed with the Drive bases
impl DriveAppFolder { pub fn new(http: GmailHttp) -> Self; }
#[async_trait] impl AppFolderStore for DriveAppFolder { /* read, write, delete */ }

#[derive(Deserialize)] struct FileList { #[serde(default)] files: Vec<FileRef> }
#[derive(Deserialize)] struct FileRef { id: String, #[serde(rename = "modifiedTime")] modified_time: Option<String> }
#[derive(Deserialize)] struct V2File { id: String, etag: String }
```

`ETag` and `AppFolderError` come from T-201. This task assumes `pub struct ETag(pub String)` and `pub enum AppFolderError { Conflict, Mail(MailError) }`.

## Algorithm

### Per-call contract (S8 for Drive)

Scope for every call: `https://www.googleapis.com/auth/drive.appdata` (S6 4; never `drive` or `drive.file`). Header `Authorization: Bearer <token>`. Timeout, egress and error mapping as T-401, plus `412` maps to `AppFolderError::Conflict` on write.

| Use | Drive call | Method and URL | Query, headers, body | Fields read |
| --- | --- | --- | --- | --- |
| Find the file | v3 `files.list` | `GET {DRIVE_V3}/files` | `spaces=appDataFolder`, `q=name = 'mailtinder-state-v1.bin' and trashed = false`, `fields=files(id,modifiedTime)`, `pageSize=10` | `files[].id`, `modifiedTime` |
| ETag | v2 `files.get` | `GET {DRIVE_V2}/files/{id}` | `fields=id,etag` | `etag` |
| Download | v3 `files.get` media | `GET {DRIVE_V3}/files/{id}` | `alt=media` | raw bytes |
| Create | v3 `files.create` multipart | `POST {DRIVE_UPLOAD_V3}/files` | `uploadType=multipart`, `fields=id`; body `multipart/related` with JSON part `{"name": APP_FILE_NAME, "parents": ["appDataFolder"]}` and an `application/octet-stream` part | `id` |
| Update with precondition | v2 `files.update` media | `PUT {DRIVE_UPLOAD_V2}/files/{id}` | `uploadType=media`, `fields=id,etag`; headers `If-Match: <etag>`, `Content-Type: application/octet-stream` | new `etag` |
| Delete | v3 `files.delete` | `DELETE {DRIVE_V3}/files/{id}` | none | none (`204`) |

Why v2 for the ETag `[DEFAULT]`: S3 requires `If-Match` writes, and Drive v3 files expose no `etag` and document no precondition on update; Drive v2 files carry `etag` and honour `If-Match` with `412 Precondition Failed`. The live contract run (T-1106) must include `app_folder_stale_etag_is_conflict` against real Drive; if Google ever stops honouring it, raise a spec change rather than silently writing without a precondition.

Rate limits: Drive returns `403` with reasons `userRateLimitExceeded` or `rateLimitExceeded`, or `429`; map to `RateLimited` per T-401. No retries in the adapter.

### Steps

1. `find(mb) -> Result<Option<String>, MailError>`: `files.list`. No files: `None`. One: its ID. More than one (two creates raced): the one with the latest `modifiedTime`, and log `outcome="app_folder_duplicates"` (count only) `[DEFAULT]`.
2. `read(mb)`:
   1. `find`; `None` gives `Ok(None)` (first use, or the user deleted the file; S10 4.2 scenario).
   2. `etag` from v2 `files.get`; then download with `alt=media`. A `404` on either (deleted in between) gives `Ok(None)`.
   3. A body over `APP_FILE_MAX_BYTES` is `MailError::Invalid("app_folder_too_large")`.
   4. Return `(bytes, ETag(etag))`. Read the ETag before the bytes, so a write between the two makes the next write fail with `Conflict` rather than overwrite unseen data.
3. `write(mb, bytes, if_match)`:
   1. `bytes.len() > APP_FILE_MAX_BYTES` gives `Mail(Invalid("app_folder_too_large"))`.
   2. `if_match == None` means "create; there must be no file yet": `find`; if a file exists, `Conflict`. Otherwise multipart create, then v2 `files.get` for its `etag` and return it.
   3. `if_match == Some(tag)`: `find`; none gives `Conflict` (it was deleted; the caller re-reads and starts again). Otherwise v2 `PUT` with `If-Match: tag`. `412` gives `Conflict`; `2xx` returns the response `etag`.
   4. Other errors map to `Mail(map_gmail_error(...))`.
4. `delete(mb)`: `find`; none gives `Ok(())`. `DELETE` the file; `404` also `Ok(())` (idempotent, AU-06 runs it during account deletion).
5. The caller's retry loop on `Conflict` (re-read, merge, write) belongs to the api (T-609), not here.

## Acceptance criteria

| ID | Behaviour (one line) |
| --- | --- |
| SR-01 AC5 | Rules are stored in the user's app folder (this store), not on our server |
| AU-05 AC4 | The app folder file can be read from one mailbox's Drive, written to another's and deleted from the first (store half) |
| AU-06 AC1 | The app folder file is deleted from the primary app folder (store half; idempotent) |

## Tests that must pass

- `app_folder_store_contract_drive` (contract: the shared suite against `fake-google`)
- `app_folder_store_contract_fake` (contract: the same suite against the in-memory fake, so the two cannot drift)
- `sr_01_ac5_write_then_read_round_trip_ciphertext` (contract)
- `au_05_ac4_copy_between_two_drives_then_delete_old` (contract: two fake mailboxes)
- `au_06_ac1_delete_is_idempotent` (contract)
- `app_folder_stale_etag_is_conflict` (contract: two writers, second gets `Conflict`)
- `app_folder_create_when_exists_is_conflict` (contract)
- `app_folder_user_deleted_file_reads_none` (contract: fake-google "file deleted by the user" scenario)
- `app_folder_duplicates_pick_latest` (contract)
- `app_folder_too_large_refused` (unit)
- `app_folder_uses_only_drive_appdata_space` (contract: fake-google records `spaces=appDataFolder` on every list and `parents=["appDataFolder"]` on create)

## Edge cases and traps

- Never write without a precondition once a file exists; a plain v3 `PATCH` would silently overwrite another tab's History.
- `If-Match` needs the v2 `etag` string exactly, quotes included; do not strip quotes.
- The `q` string has spaces and quotes; build it with the URL encoder.
- Create plus get-ETag is two calls; return the ETag from the second, not a made-up value.
- The file is ciphertext; never try to parse it here, and never log its size with a user ID beside it.
- `files.delete` on an `appDataFolder` file is a real delete. That is allowed (it is our file, not a message); INV-5 is about messages. Keep `drive.rs` the only file with a `DELETE` (T-403's structural test excludes it).
- Scope is `drive.appdata` only; a `403 insufficientPermissions` must surface as `Forbidden`, not be retried with a wider scope.

## Out of scope

- Encrypting and decrypting the file under `data_key`: T-302 types, used by T-609 and T-601.
- Moving the file on primary disconnect (orchestration): T-601.
- The merge-and-retry loop on `Conflict`: T-609.

## Done when

- The tests above pass and every required check is green (S10 10.1).
- Definition of done in S10 10.4.
- `fake-google` implements the six Drive calls in the table, including `412` on a stale `If-Match`.
