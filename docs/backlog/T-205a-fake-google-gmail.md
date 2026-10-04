# T-205a: fake-google, part 1: Gmail REST fake and control API

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M2 | sonnet | about 700 lines of code plus tests | T-203, T-204 |

Split from T-205 (index row "fake-google: Gmail and Drive HTTP fake"). T-205a is the server, token registry, control API and Gmail; T-205b adds Drive `appDataFolder`; T-206 adds OAuth and OIDC.

**Read only these spec sections:** S10 4.1 last bullet, 4.2 "Gmail" bullet and the paragraph after the list, 4.3 "Shape fixtures" (`docs/specs/S10-test-strategy.md`); the per-call tables in `docs/backlog/T-401-gmail-read-messages-and-headers.md`, `docs/backlog/T-403-gmail-trash-labels-spam-restore.md` and `docs/backlog/T-404-gmail-send-validated-mailto.md` (the calls this fake must answer); `docs/backlog/T-203-fake-mailbox-and-contract-suite.md` ("Types and signatures", `MailSeeder`). Nothing else is needed.

## Goal

`fake-google` is an `axum` HTTP server, usable as a library inside tests (port 0) and as a binary for e2e, that answers the Gmail REST calls the real adapter makes, with Gmail-shaped bodies and errors. Tests seed mailboxes from the T-204 corpus, inject failures and read back what happened through a `/__fake` control API (and the same operations as Rust methods). Any permanent delete call returns 500 and records `PERMANENT_DELETE_ATTEMPTED` (INV-5).

## Files

| Action | Path | What |
| --- | --- | --- |
| Change | `backend/crates/fake-google/Cargo.toml` | deps: `axum`, `tokio`, `serde`, `serde_json`, `base64`, `mail-parser`, `time`, `uuid`, `ports`, `testkit`, `domain` |
| Change | `backend/crates/fake-google/src/lib.rs` | `FakeGoogle`, `FakeGoogleHandle`, router assembly |
| Change | `backend/crates/fake-google/src/main.rs` | binary: bind `FAKE_GOOGLE_ADDR` (default `127.0.0.1:0`), print the bound address as one line, serve |
| Create | `backend/crates/fake-google/src/state.rs` | mailboxes, messages, labels, tokens, events, failure rules |
| Create | `backend/crates/fake-google/src/tokens.rs` | bearer token registry and scope checks |
| Create | `backend/crates/fake-google/src/errors.rs` | Gmail-shaped error bodies |
| Create | `backend/crates/fake-google/src/gmail.rs` | Gmail routes |
| Create | `backend/crates/fake-google/src/mime.rs` | `.eml` to Gmail `payload` |
| Create | `backend/crates/fake-google/src/control.rs` | `/__fake/...` routes |
| Create | `backend/crates/fake-google/src/seeder.rs` | `impl MailSeeder for FakeGoogleSeeder` |
| Create | `backend/crates/fake-google/tests/gmail_routes.rs` | route tests |

## Types and signatures

```rust
pub struct FakeGoogle;
impl FakeGoogle {
    /// Binds 127.0.0.1:0 and serves until the handle is dropped.
    pub async fn start(clock: Arc<dyn Clock>) -> Result<FakeGoogleHandle, std::io::Error>;
}
#[derive(Clone)]
pub struct FakeGoogleHandle { pub addr: SocketAddr, state: Arc<FakeState>, _shutdown: Arc<ShutdownGuard> }
impl FakeGoogleHandle {
    pub fn base_url(&self) -> url::Url;                                        // http://127.0.0.1:<port>/
    pub fn add_mailbox(&self, email: &str) -> FakeMailboxKey;                  // email must be a reserved domain
    pub fn issue_token(&self, mb: &FakeMailboxKey, scopes: &[&str], ttl: time::Duration) -> String;
    pub fn expire_token(&self, token: &str);
    pub fn seed_eml(&self, mb: &FakeMailboxKey, eml: &[u8], labels: &[&str], internal_date: OffsetDateTime) -> String; // message id
    pub fn labels_of(&self, mb: &FakeMailboxKey, id: &str) -> Option<BTreeSet<String>>;
    pub fn sent(&self, mb: &FakeMailboxKey) -> Vec<Vec<u8>>;                   // decoded raw of each send
    pub fn fail(&self, rule: FailRule);
    pub fn arm_label_create_race(&self, mb: &FakeMailboxKey);
    pub fn events(&self) -> Vec<FakeEvent>;
    pub fn permanent_delete_attempts(&self) -> u64;
    pub fn seeder(&self, mb: &FakeMailboxKey) -> Arc<dyn testkit::contract::mail_provider::MailSeeder>;
}
#[derive(Clone, Debug, PartialEq, Eq, Hash)] pub struct FakeMailboxKey(pub String); // the mailbox email
#[derive(Clone, Debug, Deserialize)]
pub struct FailRule { pub method: String, pub path_prefix: String, pub status: u16, pub reason: String,
                      pub retry_after: Option<String>, pub times: u32 }
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub enum FakeEvent {
    Request { method: String, route: &'static str, query: Vec<(String, String)> },
    PermanentDeleteAttempted { method: String, path: String },
}

pub const GMAIL_MODIFY: &str = "https://www.googleapis.com/auth/gmail.modify";
pub const GMAIL_SEND: &str = "https://www.googleapis.com/auth/gmail.send";
pub const DRIVE_APPDATA: &str = "https://www.googleapis.com/auth/drive.appdata";
```

Routes (base path `/gmail/v1/users/me`; `{userId}` other than `me` returns 400):

| Method and path | Behaviour |
| --- | --- |
| `GET /messages` | `labelIds` (repeatable, AND), `q`, `pageToken`, `maxResults` (default 100, max 500), `includeSpamTrash` (default false: exclude `TRASH` and `SPAM`). Newest first by `internalDate`, ties by ID. Body `{"messages":[{"id","threadId"}],"nextPageToken"?,"resultSizeEstimate"}`; no `messages` key when empty |
| `GET /messages/{id}` | `format` = `metadata` (only headers named by repeated `metadataHeaders`, in message order, duplicates kept), `full` (MIME tree as `payload` with `mimeType`, `headers`, `body{size,data}` base64url, `parts`), `minimal` (no payload). Any other `format` gives 400 `invalidArgument`. Always `id`, `threadId`, `labelIds`, `snippet` (first 100 chars of text), `internalDate` (string of ms), `sizeEstimate` |
| `POST /messages/{id}/modify` | body `{"addLabelIds":[],"removeLabelIds":[]}`; unknown label ID gives 400; returns the message in `minimal` form |
| `POST /messages/{id}/trash` | adds `TRASH`, removes `INBOX` `[ASSUMES]`; returns `minimal` |
| `POST /messages/{id}/untrash` | removes `TRASH` (does not re-add `INBOX`) `[ASSUMES]`; returns `minimal` |
| `POST /messages/send` | body `{"raw": base64url}`; needs `gmail.send` or `gmail.modify`; stores a new message labelled `SENT` with the decoded bytes; returns `{"id","threadId","labelIds":["SENT"]}` |
| `GET /labels` | `{"labels":[{"id","name","type"}]}` (system then user labels) |
| `GET /labels/{id}` | `{"id","name","type","messagesTotal","messagesUnread"}` |
| `POST /labels` | `{"name",...}`; name clash ignoring ASCII case gives 409 `alreadyExists`; else creates `Label_<n>` |
| `GET /profile` | `{"emailAddress","messagesTotal","threadsTotal","historyId"}` |
| `DELETE /messages/{id}`, `POST /messages/batchDelete`, `DELETE /threads/{id}` | 500 `backendError` and record `PermanentDeleteAttempted` (INV-5). Nothing is deleted |

`q` subset: whitespace-separated terms `after:<unix seconds>`, `before:<unix seconds>`, `from:<address>`, `list:<list-id>`; any other term gives 400 `invalidArgument` so an adapter bug shows up (T-602a may extend the parser).

Control API (all under `/__fake`, JSON, no auth, served only by this binary):

| Route | Does |
| --- | --- |
| `POST /__fake/gmail/mailboxes` `{email}` | add mailbox |
| `POST /__fake/tokens` `{email, scopes, ttl_s}` | issue a bearer token, returns `{access_token}` |
| `POST /__fake/tokens/expire` `{access_token}` | later calls with it get 401 |
| `POST /__fake/gmail/messages` `{email, eml_base64, labels, internal_date}` | seed one message, returns `{id}` |
| `POST /__fake/gmail/seed-corpus` `{email}` | seeds every T-204 corpus case with `INBOX` |
| `GET /__fake/gmail/messages/{email}/{id}` | `{labelIds}` read back |
| `GET /__fake/gmail/sent/{email}` | `{raw_base64: [...]}` |
| `POST /__fake/fail` | add a `FailRule` |
| `POST /__fake/gmail/labels-create-race` `{email}` | next `POST /labels` creates the label and still answers 409 |
| `GET /__fake/events` | every `FakeEvent` |
| `POST /__fake/reset` | clear everything |

## Algorithm

1. State: `Mutex<FakeState>` with mailboxes keyed by email; each holds messages (`id`, raw bytes, parsed tree, labels, internal date), labels (`INBOX`, `SENT`, `TRASH`, `SPAM`, `UNREAD`, `STARRED`, `IMPORTANT`, `DRAFT`, `CATEGORY_PERSONAL`, then user labels), a next-ID counter (IDs are 16 lower-case hex chars from the counter, like Gmail's), and a sent list.
2. Auth middleware on every non-control route: header `Authorization: Bearer <t>`. Missing, unknown or expired (by the injected `Clock`) gives 401 with reason `authError`. Wrong scope gives 403 `insufficientPermissions`. A token belongs to one mailbox; `users/me` means that mailbox.
3. Failure rules: before handling, find the first rule whose method matches and whose request path starts with `path_prefix` and `times > 0`; decrement and answer with `status`, the Gmail error body with `reason`, and `Retry-After` if set.
4. Error body shape: `{"error":{"code":<status>,"message":"<fixed text per status>","status":"<GRPC name>","errors":[{"domain":"global","reason":"<reason>","message":"<fixed text>"}]}}`. Status names: 400 `INVALID_ARGUMENT`, 401 `UNAUTHENTICATED`, 403 `PERMISSION_DENIED`, 404 `NOT_FOUND`, 409 `ALREADY_EXISTS`, 429 `RESOURCE_EXHAUSTED`, 500 `INTERNAL`, 503 `UNAVAILABLE`.
5. `mime.rs`: parse with `mail-parser`; build `payload` recursively: `partId`, `mimeType`, `filename`, `headers` (name and decoded value, message order), `body` (`size`, and `data` base64url without padding for leaf parts; `attachmentId` instead of data for parts with `Content-Disposition: attachment`). Never fetch anything a message refers to.
6. Paging: `pageToken` = `"p" + offset` as decimal; anything else gives 400.
7. Record a `Request` event for every Gmail call with the route template (for example `messages.get`) and the query pairs. Fake data is synthetic, so recording query values is allowed here.
8. `FakeGoogleSeeder` implements `MailSeeder`: `seed` builds a small `.eml` from `SeedMessage` (or uses a T-204 case's bytes) and calls `seed_eml`; `labels_of` and `sent` read state; `permanent_delete_attempts` counts events.
9. Shutdown: dropping the last handle stops the server (graceful shutdown on a `oneshot`).

## Acceptance criteria

| ID | Behaviour (one line) |
| --- | --- |
| INV-5 | Any Gmail permanent delete call returns 500, deletes nothing and records `PERMANENT_DELETE_ATTEMPTED` |

## Tests that must pass

- `inv_5_fake_google_delete_routes_return_500_and_record` (integration: all three delete routes).
- `fake_google_list_newest_first_and_pages` (integration).
- `fake_google_list_q_after_before_from_list` (integration) and `fake_google_unknown_q_term_is_400`.
- `fake_google_get_metadata_returns_only_named_headers_in_order_with_duplicates` (integration).
- `fake_google_get_full_payload_tree_and_attachment_id` (integration).
- `fake_google_unknown_format_is_400` (integration).
- `fake_google_missing_or_expired_token_is_401` and `fake_google_wrong_scope_is_403` (integration).
- `fake_google_fail_rule_shapes_429_with_retry_after` (integration).
- `fake_google_trash_untrash_modify_labels` (integration).
- `fake_google_label_create_race_answers_409_but_creates` (integration).
- `fake_google_send_stores_raw_in_sent` (integration).
- `fake_google_seed_corpus_seeds_every_case` (integration with T-204).
- `fake_google_binary_prints_bound_address` (integration: run the binary with `127.0.0.1:0`, read the first stdout line).

## Edge cases and traps

- Tests bind port 0 and read the address back (S10 3.1); never a fixed port.
- `internalDate` is a string of milliseconds; `historyId` is a string too.
- Return `labelIds` in a stable order (system labels first in the order above, then user labels by ID) so tests do not flake.
- The control API must not exist in any production binary; it lives only in this crate (S10 4.2).
- Add mailbox emails at reserved domains only; reject others with 400 so a test cannot seed a real-looking address.
- `mail-parser` is MIT or Apache-2.0. Do not use `mailparse`: its licence (0BSD) is not on the `deny.toml` allowlist.
- Do not hold the state `Mutex` across an `.await`.
- The fake ignores the `fields` parameter and returns full objects; adapters must still tolerate extra fields (they do not use `deny_unknown_fields` on Gmail responses).
- `println!` is banned by Clippy (T-003); print the bound address with `std::io::Write` on a locked stdout handle in `main.rs`.

## Out of scope

- Drive (T-205b), OAuth and OIDC (T-206), the extended query parser and label patch and delete (T-602a).

## Done when

- The tests above pass and every required check is green (S10 10.1).
- Definition of done in S10 10.4.
- The route table above is copied into `fake-google/src/gmail.rs` as a module doc comment.
