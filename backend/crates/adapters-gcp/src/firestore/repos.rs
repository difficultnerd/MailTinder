//! One `impl` per repository trait over the Firestore REST client.

use std::sync::Arc;

use async_trait::async_trait;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use domain::{InviteStatus, JobId, MailboxId, Provider, ProviderSubjectId, UserId};
use ports::store::{
    BakeoffSnapshotRecord, BakeoffSnapshotRepo, ClassifierEvalRecord, ClassifierEvalRepo,
    ClassifiersConfig, ConfigRepo, EmailLookupHash, EvalId, InviteId, InviteRecord, InviteRepo,
    InviteRequestId, InviteRequestRecord, InviteRequestRepo, JobRecord, JobRepo, Keyed,
    ListKeyHash, MailboxRecord, MailboxRepo, NeedsAttentionId, NeedsAttentionRecord,
    NeedsAttentionRepo, Page, PageRequest, Precondition, RateLimitKey, RateLimitRepo, Repo,
    SessionHash, SessionRecord, SessionRepo, SnapshotId, StoreCursor, StoreError, UserRecord,
    UserRepo, Version, Versioned, MAX_PAGE,
};
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::{json, Value};
use time::OffsetDateTime;

use super::rest::{Document, Firestore, Write};
use super::value::{from_fields, to_fields};
use crate::gcp_http::GcpError;

/// Renders a record key as a Firestore document ID.
trait DocKey {
    fn doc_id(&self) -> String;
}
impl DocKey for UserId {
    fn doc_id(&self) -> String {
        self.to_string()
    }
}
impl DocKey for MailboxId {
    fn doc_id(&self) -> String {
        self.to_string()
    }
}
impl DocKey for JobId {
    fn doc_id(&self) -> String {
        self.to_string()
    }
}
impl DocKey for InviteId {
    fn doc_id(&self) -> String {
        self.0.to_string()
    }
}
impl DocKey for InviteRequestId {
    fn doc_id(&self) -> String {
        self.0.to_string()
    }
}
impl DocKey for NeedsAttentionId {
    fn doc_id(&self) -> String {
        self.0.to_string()
    }
}
impl DocKey for EvalId {
    fn doc_id(&self) -> String {
        self.0.to_string()
    }
}
impl DocKey for SnapshotId {
    fn doc_id(&self) -> String {
        self.0.to_string()
    }
}
impl DocKey for SessionHash {
    fn doc_id(&self) -> String {
        self.to_hex()
    }
}

/// A generic Firestore-backed repository for a single collection.
struct FsRepo<R> {
    fs: Arc<Firestore>,
    collection: &'static str,
    _marker: std::marker::PhantomData<R>,
}

impl<R: Keyed + Serialize + DeserializeOwned + Clone> FsRepo<R> {
    fn new(fs: Arc<Firestore>, collection: &'static str) -> Self {
        Self {
            fs,
            collection,
            _marker: std::marker::PhantomData,
        }
    }

    fn doc_name(&self, key: &R::Key) -> String
    where
        R::Key: DocKey,
    {
        self.fs.doc_name(self.collection, &key.doc_id())
    }

    async fn get(&self, key: &R::Key) -> Result<Option<Versioned<R>>, StoreError>
    where
        R::Key: DocKey,
    {
        let name = self.doc_name(key);
        match self.fs.get(&name).await {
            Ok(Some(doc)) => Ok(Some(decode_doc::<R>(&doc)?)),
            Ok(None) => Ok(None),
            Err(e) => Err(map_err(e)),
        }
    }

    async fn put(&self, record: &R, pre: Precondition) -> Result<Version, StoreError>
    where
        R::Key: DocKey,
    {
        let name = self.doc_name(&record.key());
        let fields =
            to_fields(&serde_json::to_value(record).map_err(|_| StoreError::Invalid("serialise"))?)
                .map_err(|_| StoreError::Invalid("fields"))?;
        let write = Write {
            update: Some((name, fields)),
            current_document: precondition_json(&pre),
            ..Default::default()
        };
        let resp = self.fs.commit(&[write]).await.map_err(map_err)?;
        let version = write_result_version(&resp, 0)?;
        Ok(version)
    }

    async fn delete(&self, key: &R::Key, pre: Precondition) -> Result<(), StoreError>
    where
        R::Key: DocKey,
    {
        let name = self.doc_name(key);
        let write = Write {
            delete: Some(name),
            current_document: precondition_json(&pre),
            ..Default::default()
        };
        self.fs.commit(&[write]).await.map_err(map_err)?;
        Ok(())
    }
}

fn decode_doc<R: DeserializeOwned>(doc: &Document) -> Result<Versioned<R>, StoreError> {
    let value = from_fields(&doc.fields).map_err(|_| StoreError::Corrupt("fields"))?;
    let record: R = serde_json::from_value(value).map_err(|_| StoreError::Corrupt("record"))?;
    Ok(Versioned {
        record,
        version: Version(doc.update_time.clone()),
    })
}

/// The `currentDocument` precondition, or `None` when there is no precondition
/// (the spec: `Precondition::None` omits `currentDocument`).
fn precondition_json(pre: &Precondition) -> Option<Value> {
    match pre {
        Precondition::None => None,
        Precondition::MustNotExist => Some(json!({ "exists": false })),
        Precondition::MustExist => Some(json!({ "exists": true })),
        Precondition::Matches(v) => Some(json!({ "updateTime": v.0 })),
    }
}

fn write_result_version(resp: &Value, index: usize) -> Result<Version, StoreError> {
    let update_time = resp
        .pointer(&format!("/writeResults/{index}/updateTime"))
        .and_then(|v| v.as_str())
        .ok_or(StoreError::Corrupt("write result"))?;
    Ok(Version(update_time.to_owned()))
}

fn map_err(e: GcpError) -> StoreError {
    match e {
        GcpError::AlreadyExists => StoreError::AlreadyExists,
        GcpError::FailedPrecondition | GcpError::NotFound => StoreError::PreconditionFailed,
        GcpError::Unavailable | GcpError::Aborted => StoreError::Unavailable,
        GcpError::BadResponse => StoreError::Corrupt("response"),
        _ => StoreError::Unavailable,
    }
}

/// Build a `fieldFilter` for an equality on a field.
fn eq_filter(field: &str, value: Value) -> Value {
    json!({
        "fieldFilter": {
            "field": { "fieldPath": field },
            "op": "EQUAL",
            "value": value,
        }
    })
}

/// Build a `fieldFilter` for a comparison on a field.
fn cmp_filter(field: &str, op: &str, value: Value) -> Value {
    json!({
        "fieldFilter": {
            "field": { "fieldPath": field },
            "op": op,
            "value": value,
        }
    })
}

/// Combine filters with `AND`.
fn and(filters: Vec<Value>) -> Value {
    if filters.len() == 1 {
        filters.into_iter().next().unwrap_or(Value::Null)
    } else {
        json!({ "compositeFilter": { "op": "AND", "filters": filters } })
    }
}

/// A Firestore `Value` for a string (used in filters).
fn string_value(s: &str) -> Value {
    json!({ "stringValue": s })
}

/// A Firestore `Value` for a timestamp (used in filters).
fn timestamp_value(dt: OffsetDateTime) -> Value {
    let formatted = dt
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_owned());
    json!({ "timestampValue": formatted })
}

/// Build a structured query with `from`, optional `where`, `orderBy`, `limit`
/// and `startAt`.
fn query(
    collection: &str,
    filter: Option<Value>,
    order_by: Vec<(&str, &str)>,
    limit: Option<u32>,
    start_at: Option<Value>,
) -> Value {
    let mut q = json!({ "from": [{ "collectionId": collection }] });
    if let Some(f) = filter {
        q["where"] = f;
    }
    if !order_by.is_empty() {
        let fields: Vec<Value> = order_by
            .iter()
            .map(|(f, dir)| json!({ "field": { "fieldPath": f }, "direction": dir }))
            .collect();
        q["orderBy"] = Value::Array(fields);
    }
    if let Some(l) = limit {
        q["limit"] = json!(l);
    }
    if let Some(sa) = start_at {
        q["startAt"] = sa;
    }
    q
}

/// Encode a page cursor: unpadded base64url of `[<last order value>, "<last doc name>"]`.
fn encode_cursor(order_value: Value, doc_name: &str) -> StoreCursor {
    let payload = json!([order_value, doc_name]);
    let bytes = serde_json::to_vec(&payload).unwrap_or_default();
    StoreCursor(URL_SAFE_NO_PAD.encode(bytes))
}

/// Decode a page cursor back into `(order_value, doc_name)`.
fn decode_cursor(cursor: &StoreCursor) -> Option<(Value, String)> {
    let bytes = URL_SAFE_NO_PAD.decode(cursor.0.as_bytes()).ok()?;
    let payload: Value = serde_json::from_slice(&bytes).ok()?;
    let arr = payload.as_array()?;
    let order = arr.first()?.clone();
    let name = arr.get(1)?.as_str()?.to_owned();
    Some((order, name))
}

/// Run a query and page the results by `limit + 1` to know if `next` exists.
async fn run_paged<R: DeserializeOwned + Clone>(
    fs: &Arc<Firestore>,
    collection: &str,
    filter: Option<Value>,
    order_by: Vec<(&str, &str)>,
    page: PageRequest,
    order_value: impl Fn(&R) -> Value,
) -> Result<Page<Versioned<R>>, StoreError> {
    let limit = page.limit.clamp(1, MAX_PAGE);
    let start_at = page.after.as_ref().and_then(|c| {
            decode_cursor(c).map(|(ov, name)| json!({ "values": [ov, json!({ "referenceValue": name })], "before": false }))
        });
    // Order as the method states, then `__name__` ascending as the tiebreak
    // (T-301 step 7), so the cursor's two values match the orderBy fields.
    let mut order = order_by;
    order.push(("__name__", "ASCENDING"));
    let q = query(collection, filter, order, Some(limit + 1), start_at);
    let docs = fs.run_query(collection, q).await.map_err(map_err)?;
    let mut items: Vec<Versioned<R>> =
        docs.iter().map(decode_doc::<R>).collect::<Result<_, _>>()?;
    let has_more = items.len() > limit as usize;
    let next = if has_more {
        let last = &items[limit as usize - 1];
        let name = docs[limit as usize - 1].name.clone();
        Some(encode_cursor(order_value(&last.record), &name))
    } else {
        None
    };
    items.truncate(limit as usize);
    Ok(Page { items, next })
}

// ---------- users ----------

pub struct UsersImpl {
    repo: FsRepo<UserRecord>,
}
impl UsersImpl {
    pub fn new(fs: Arc<Firestore>) -> Self {
        Self {
            repo: FsRepo::new(fs, "users"),
        }
    }
}
#[async_trait]
impl Repo<UserId, UserRecord> for UsersImpl {
    async fn get(&self, key: &UserId) -> Result<Option<Versioned<UserRecord>>, StoreError> {
        self.repo.get(key).await
    }
    async fn put(&self, record: &UserRecord, pre: Precondition) -> Result<Version, StoreError> {
        self.repo.put(record, pre).await
    }
    async fn delete(&self, key: &UserId, pre: Precondition) -> Result<(), StoreError> {
        self.repo.delete(key, pre).await
    }
}
#[async_trait]
impl UserRepo for UsersImpl {
    async fn list(&self, page: PageRequest) -> Result<Page<Versioned<UserRecord>>, StoreError> {
        run_paged(
            &self.repo.fs,
            "users",
            None,
            vec![("created_at", "ASCENDING")],
            page,
            |r: &UserRecord| timestamp_value(r.created_at),
        )
        .await
    }
}

// ---------- mailboxes ----------

pub struct MailboxesImpl {
    repo: FsRepo<MailboxRecord>,
}
impl MailboxesImpl {
    pub fn new(fs: Arc<Firestore>) -> Self {
        Self {
            repo: FsRepo::new(fs, "mailboxes"),
        }
    }
}
#[async_trait]
impl Repo<MailboxId, MailboxRecord> for MailboxesImpl {
    async fn get(&self, key: &MailboxId) -> Result<Option<Versioned<MailboxRecord>>, StoreError> {
        self.repo.get(key).await
    }
    async fn put(&self, record: &MailboxRecord, pre: Precondition) -> Result<Version, StoreError> {
        self.repo.put(record, pre).await
    }
    async fn delete(&self, key: &MailboxId, pre: Precondition) -> Result<(), StoreError> {
        self.repo.delete(key, pre).await
    }
}
#[async_trait]
impl MailboxRepo for MailboxesImpl {
    async fn by_user(&self, user: &UserId) -> Result<Vec<Versioned<MailboxRecord>>, StoreError> {
        let filter = eq_filter("user_id", string_value(&user.to_string()));
        let q = query(
            "mailboxes",
            Some(filter),
            vec![("linked_at", "ASCENDING")],
            None,
            None,
        );
        let docs = self
            .repo
            .fs
            .run_query("mailboxes", q)
            .await
            .map_err(map_err)?;
        docs.iter().map(decode_doc::<MailboxRecord>).collect()
    }
    async fn by_subject(
        &self,
        provider: Provider,
        subject: &ProviderSubjectId,
    ) -> Result<Option<Versioned<MailboxRecord>>, StoreError> {
        let provider_str = match provider {
            Provider::Gmail => "gmail",
        };
        let filter = and(vec![
            eq_filter("provider", string_value(provider_str)),
            eq_filter("provider_subject_id", string_value(subject.as_str())),
        ]);
        let q = query("mailboxes", Some(filter), vec![], Some(1), None);
        let docs = self
            .repo
            .fs
            .run_query("mailboxes", q)
            .await
            .map_err(map_err)?;
        Ok(docs.first().map(decode_doc::<MailboxRecord>).transpose()?)
    }
    async fn delete_all_for_user(&self, user: &UserId) -> Result<u64, StoreError> {
        bulk_delete_by_filter(
            &self.repo.fs,
            "mailboxes",
            eq_filter("user_id", string_value(&user.to_string())),
        )
        .await
    }
}

// ---------- invites ----------

pub struct InvitesImpl {
    repo: FsRepo<InviteRecord>,
}
impl InvitesImpl {
    pub fn new(fs: Arc<Firestore>) -> Self {
        Self {
            repo: FsRepo::new(fs, "invites"),
        }
    }
}
#[async_trait]
impl Repo<InviteId, InviteRecord> for InvitesImpl {
    async fn get(&self, key: &InviteId) -> Result<Option<Versioned<InviteRecord>>, StoreError> {
        self.repo.get(key).await
    }
    async fn put(&self, record: &InviteRecord, pre: Precondition) -> Result<Version, StoreError> {
        self.repo.put(record, pre).await
    }
    async fn delete(&self, key: &InviteId, pre: Precondition) -> Result<(), StoreError> {
        self.repo.delete(key, pre).await
    }
}
#[async_trait]
impl InviteRepo for InvitesImpl {
    async fn by_token_hash(
        &self,
        hash: &ports::store::Sha256Hash,
    ) -> Result<Option<Versioned<InviteRecord>>, StoreError> {
        let filter = eq_filter("token_hash", string_value(&hex(hash.0)));
        let q = query("invites", Some(filter), vec![], Some(1), None);
        let docs = self
            .repo
            .fs
            .run_query("invites", q)
            .await
            .map_err(map_err)?;
        Ok(docs.first().map(decode_doc::<InviteRecord>).transpose()?)
    }
    async fn by_email_lookup(
        &self,
        hash: &EmailLookupHash,
    ) -> Result<Vec<Versioned<InviteRecord>>, StoreError> {
        let filter = eq_filter("email_lookup", string_value(&hex(hash.0)));
        let q = query("invites", Some(filter), vec![], None, None);
        let docs = self
            .repo
            .fs
            .run_query("invites", q)
            .await
            .map_err(map_err)?;
        docs.iter().map(decode_doc::<InviteRecord>).collect()
    }
    async fn list(
        &self,
        status: Option<InviteStatus>,
        page: PageRequest,
    ) -> Result<Page<Versioned<InviteRecord>>, StoreError> {
        let filter = status.map(|s| {
            let s = match s {
                InviteStatus::Pending => "pending",
                InviteStatus::Used => "used",
                InviteStatus::Revoked => "revoked",
                InviteStatus::Expired => "expired",
            };
            eq_filter("status", string_value(s))
        });
        run_paged(
            &self.repo.fs,
            "invites",
            filter,
            vec![("created_at", "DESCENDING")],
            page,
            |r: &InviteRecord| timestamp_value(r.created_at),
        )
        .await
    }
    async fn purge_due(
        &self,
        now: OffsetDateTime,
        limit: u32,
    ) -> Result<Vec<InviteId>, StoreError> {
        let filter = cmp_filter("purge_at", "LESS_THAN_OR_EQUAL", timestamp_value(now));
        let q = query(
            "invites",
            Some(filter),
            vec![("purge_at", "ASCENDING")],
            Some(limit),
            None,
        );
        let docs = self
            .repo
            .fs
            .run_query("invites", q)
            .await
            .map_err(map_err)?;
        docs.iter()
            .map(|d| {
                let v = from_fields(&d.fields).map_err(|_| StoreError::Corrupt("fields"))?;
                let r: InviteRecord =
                    serde_json::from_value(v).map_err(|_| StoreError::Corrupt("record"))?;
                Ok(r.invite_id)
            })
            .collect()
    }
}

// ---------- invite_requests ----------

pub struct InviteRequestsImpl {
    repo: FsRepo<InviteRequestRecord>,
}
impl InviteRequestsImpl {
    pub fn new(fs: Arc<Firestore>) -> Self {
        Self {
            repo: FsRepo::new(fs, "invite_requests"),
        }
    }
}
#[async_trait]
impl Repo<InviteRequestId, InviteRequestRecord> for InviteRequestsImpl {
    async fn get(
        &self,
        key: &InviteRequestId,
    ) -> Result<Option<Versioned<InviteRequestRecord>>, StoreError> {
        self.repo.get(key).await
    }
    async fn put(
        &self,
        record: &InviteRequestRecord,
        pre: Precondition,
    ) -> Result<Version, StoreError> {
        self.repo.put(record, pre).await
    }
    async fn delete(&self, key: &InviteRequestId, pre: Precondition) -> Result<(), StoreError> {
        self.repo.delete(key, pre).await
    }
}
#[async_trait]
impl InviteRequestRepo for InviteRequestsImpl {
    async fn by_email_lookup(
        &self,
        hash: &EmailLookupHash,
    ) -> Result<Option<Versioned<InviteRequestRecord>>, StoreError> {
        let filter = eq_filter("email_lookup", string_value(&hex(hash.0)));
        let q = query("invite_requests", Some(filter), vec![], Some(1), None);
        let docs = self
            .repo
            .fs
            .run_query("invite_requests", q)
            .await
            .map_err(map_err)?;
        Ok(docs
            .first()
            .map(decode_doc::<InviteRequestRecord>)
            .transpose()?)
    }
    async fn list(
        &self,
        page: PageRequest,
    ) -> Result<Page<Versioned<InviteRequestRecord>>, StoreError> {
        run_paged(
            &self.repo.fs,
            "invite_requests",
            None,
            vec![("created_at", "ASCENDING")],
            page,
            |r: &InviteRequestRecord| timestamp_value(r.created_at),
        )
        .await
    }
}

// ---------- jobs ----------

pub struct JobsImpl {
    repo: FsRepo<JobRecord>,
}
impl JobsImpl {
    pub fn new(fs: Arc<Firestore>) -> Self {
        Self {
            repo: FsRepo::new(fs, "jobs"),
        }
    }
}
#[async_trait]
impl Repo<JobId, JobRecord> for JobsImpl {
    async fn get(&self, key: &JobId) -> Result<Option<Versioned<JobRecord>>, StoreError> {
        self.repo.get(key).await
    }
    async fn put(&self, record: &JobRecord, pre: Precondition) -> Result<Version, StoreError> {
        self.repo.put(record, pre).await
    }
    async fn delete(&self, key: &JobId, pre: Precondition) -> Result<(), StoreError> {
        self.repo.delete(key, pre).await
    }
}
#[async_trait]
impl JobRepo for JobsImpl {
    async fn by_mailbox(
        &self,
        mailbox: &MailboxId,
    ) -> Result<Vec<Versioned<JobRecord>>, StoreError> {
        let filter = eq_filter("mailbox_id", string_value(&mailbox.to_string()));
        let q = query("jobs", Some(filter), vec![], None, None);
        let docs = self.repo.fs.run_query("jobs", q).await.map_err(map_err)?;
        docs.iter().map(decode_doc::<JobRecord>).collect()
    }
    async fn by_user_with_outcome(
        &self,
        user: &UserId,
        limit: u32,
    ) -> Result<Vec<Versioned<JobRecord>>, StoreError> {
        let filter = and(vec![
            eq_filter("user_id", string_value(&user.to_string())),
            json!({ "unaryFilter": { "field": { "fieldPath": "outcome" }, "op": "IS_NOT_NULL" } }),
        ]);
        let q = query(
            "jobs",
            Some(filter),
            vec![("due_at", "ASCENDING")],
            Some(limit),
            None,
        );
        let docs = self.repo.fs.run_query("jobs", q).await.map_err(map_err)?;
        docs.iter().map(decode_doc::<JobRecord>).collect()
    }
    async fn queued_for_list(
        &self,
        user: &UserId,
        list: &ListKeyHash,
    ) -> Result<Vec<Versioned<JobRecord>>, StoreError> {
        let filter = and(vec![
            eq_filter("user_id", string_value(&user.to_string())),
            eq_filter("list_key_hash", string_value(&hex(list.0))),
            eq_filter("status", string_value("queued")),
        ]);
        let q = query("jobs", Some(filter), vec![], None, None);
        let docs = self.repo.fs.run_query("jobs", q).await.map_err(map_err)?;
        docs.iter().map(decode_doc::<JobRecord>).collect()
    }
    async fn expires_by(
        &self,
        now: OffsetDateTime,
        limit: u32,
    ) -> Result<Vec<Versioned<JobRecord>>, StoreError> {
        let filter = cmp_filter("expires_at", "LESS_THAN_OR_EQUAL", timestamp_value(now));
        let q = query(
            "jobs",
            Some(filter),
            vec![("expires_at", "ASCENDING")],
            Some(limit),
            None,
        );
        let docs = self.repo.fs.run_query("jobs", q).await.map_err(map_err)?;
        docs.iter().map(decode_doc::<JobRecord>).collect()
    }
    async fn delete_all_for_user(&self, user: &UserId) -> Result<u64, StoreError> {
        bulk_delete_by_filter(
            &self.repo.fs,
            "jobs",
            eq_filter("user_id", string_value(&user.to_string())),
        )
        .await
    }
}

// ---------- needs_attention ----------

pub struct NeedsAttentionImpl {
    repo: FsRepo<NeedsAttentionRecord>,
}
impl NeedsAttentionImpl {
    pub fn new(fs: Arc<Firestore>) -> Self {
        Self {
            repo: FsRepo::new(fs, "needs_attention"),
        }
    }
}
#[async_trait]
impl Repo<NeedsAttentionId, NeedsAttentionRecord> for NeedsAttentionImpl {
    async fn get(
        &self,
        key: &NeedsAttentionId,
    ) -> Result<Option<Versioned<NeedsAttentionRecord>>, StoreError> {
        self.repo.get(key).await
    }
    async fn put(
        &self,
        record: &NeedsAttentionRecord,
        pre: Precondition,
    ) -> Result<Version, StoreError> {
        self.repo.put(record, pre).await
    }
    async fn delete(&self, key: &NeedsAttentionId, pre: Precondition) -> Result<(), StoreError> {
        self.repo.delete(key, pre).await
    }
}
#[async_trait]
impl NeedsAttentionRepo for NeedsAttentionImpl {
    async fn by_user(
        &self,
        user: &UserId,
        page: PageRequest,
    ) -> Result<Page<Versioned<NeedsAttentionRecord>>, StoreError> {
        let filter = eq_filter("user_id", string_value(&user.to_string()));
        run_paged(
            &self.repo.fs,
            "needs_attention",
            Some(filter),
            vec![("created_at", "DESCENDING")],
            page,
            |r: &NeedsAttentionRecord| timestamp_value(r.created_at),
        )
        .await
    }
    async fn count_for_user(&self, user: &UserId) -> Result<u64, StoreError> {
        let filter = eq_filter("user_id", string_value(&user.to_string()));
        let q = query("needs_attention", Some(filter), vec![], None, None);
        let docs = self
            .repo
            .fs
            .run_query("needs_attention", q)
            .await
            .map_err(map_err)?;
        Ok(docs.len() as u64)
    }
    async fn expires_by(
        &self,
        now: OffsetDateTime,
        limit: u32,
    ) -> Result<Vec<NeedsAttentionId>, StoreError> {
        let filter = cmp_filter("expires_at", "LESS_THAN_OR_EQUAL", timestamp_value(now));
        let q = query(
            "needs_attention",
            Some(filter),
            vec![("expires_at", "ASCENDING")],
            Some(limit),
            None,
        );
        let docs = self
            .repo
            .fs
            .run_query("needs_attention", q)
            .await
            .map_err(map_err)?;
        docs.iter()
            .map(|d| {
                let v = from_fields(&d.fields).map_err(|_| StoreError::Corrupt("fields"))?;
                let r: NeedsAttentionRecord =
                    serde_json::from_value(v).map_err(|_| StoreError::Corrupt("record"))?;
                Ok(r.item_id)
            })
            .collect()
    }
    async fn delete_all_for_user(&self, user: &UserId) -> Result<u64, StoreError> {
        bulk_delete_by_filter(
            &self.repo.fs,
            "needs_attention",
            eq_filter("user_id", string_value(&user.to_string())),
        )
        .await
    }
}

// ---------- sessions ----------

pub struct SessionsImpl {
    repo: FsRepo<SessionRecord>,
}
impl SessionsImpl {
    pub fn new(fs: Arc<Firestore>) -> Self {
        Self {
            repo: FsRepo::new(fs, "sessions"),
        }
    }
}
#[async_trait]
impl Repo<SessionHash, SessionRecord> for SessionsImpl {
    async fn get(&self, key: &SessionHash) -> Result<Option<Versioned<SessionRecord>>, StoreError> {
        self.repo.get(key).await
    }
    async fn put(&self, record: &SessionRecord, pre: Precondition) -> Result<Version, StoreError> {
        self.repo.put(record, pre).await
    }
    async fn delete(&self, key: &SessionHash, pre: Precondition) -> Result<(), StoreError> {
        self.repo.delete(key, pre).await
    }
}
#[async_trait]
impl SessionRepo for SessionsImpl {
    async fn by_user(&self, user: &UserId) -> Result<Vec<Versioned<SessionRecord>>, StoreError> {
        let filter = eq_filter("user_id", string_value(&user.to_string()));
        let q = query("sessions", Some(filter), vec![], None, None);
        let docs = self
            .repo
            .fs
            .run_query("sessions", q)
            .await
            .map_err(map_err)?;
        docs.iter().map(decode_doc::<SessionRecord>).collect()
    }
    async fn expires_by(
        &self,
        now: OffsetDateTime,
        limit: u32,
    ) -> Result<Vec<SessionHash>, StoreError> {
        let filter = cmp_filter("expires_at", "LESS_THAN_OR_EQUAL", timestamp_value(now));
        let q = query(
            "sessions",
            Some(filter),
            vec![("expires_at", "ASCENDING")],
            Some(limit),
            None,
        );
        let docs = self
            .repo
            .fs
            .run_query("sessions", q)
            .await
            .map_err(map_err)?;
        docs.iter()
            .map(|d| {
                let v = from_fields(&d.fields).map_err(|_| StoreError::Corrupt("fields"))?;
                let r: SessionRecord =
                    serde_json::from_value(v).map_err(|_| StoreError::Corrupt("record"))?;
                Ok(r.session_hash)
            })
            .collect()
    }
    async fn delete_all_for_user(&self, user: &UserId) -> Result<u64, StoreError> {
        bulk_delete_by_filter(
            &self.repo.fs,
            "sessions",
            eq_filter("user_id", string_value(&user.to_string())),
        )
        .await
    }
}

// ---------- classifier_eval ----------

pub struct ClassifierEvalImpl {
    repo: FsRepo<ClassifierEvalRecord>,
}
impl ClassifierEvalImpl {
    pub fn new(fs: Arc<Firestore>) -> Self {
        Self {
            repo: FsRepo::new(fs, "classifier_eval"),
        }
    }
}
#[async_trait]
impl Repo<EvalId, ClassifierEvalRecord> for ClassifierEvalImpl {
    async fn get(
        &self,
        key: &EvalId,
    ) -> Result<Option<Versioned<ClassifierEvalRecord>>, StoreError> {
        self.repo.get(key).await
    }
    async fn put(
        &self,
        record: &ClassifierEvalRecord,
        pre: Precondition,
    ) -> Result<Version, StoreError> {
        self.repo.put(record, pre).await
    }
    async fn delete(&self, key: &EvalId, pre: Precondition) -> Result<(), StoreError> {
        self.repo.delete(key, pre).await
    }
}
#[async_trait]
impl ClassifierEvalRepo for ClassifierEvalImpl {
    async fn range(
        &self,
        from: OffsetDateTime,
        to: OffsetDateTime,
        page: PageRequest,
    ) -> Result<Page<ClassifierEvalRecord>, StoreError> {
        let filter = and(vec![
            cmp_filter("created_at", "GREATER_THAN_OR_EQUAL", timestamp_value(from)),
            cmp_filter("created_at", "LESS_THAN", timestamp_value(to)),
        ]);
        let paged = run_paged(
            &self.repo.fs,
            "classifier_eval",
            Some(filter),
            vec![("created_at", "ASCENDING")],
            page,
            |r: &ClassifierEvalRecord| timestamp_value(r.created_at),
        )
        .await?;
        Ok(Page {
            items: paged.items.into_iter().map(|v| v.record).collect(),
            next: paged.next,
        })
    }
    async fn delete_for_users(
        &self,
        ids: &[ports::store::UserPseudoId],
    ) -> Result<u64, StoreError> {
        let mut total = 0u64;
        for id in ids {
            let filter = eq_filter("user_pseudo_id", string_value(&id.0));
            total += bulk_delete_by_filter(&self.repo.fs, "classifier_eval", filter).await?;
        }
        Ok(total)
    }
    async fn expires_by(&self, now: OffsetDateTime, limit: u32) -> Result<Vec<EvalId>, StoreError> {
        let filter = cmp_filter("expires_at", "LESS_THAN_OR_EQUAL", timestamp_value(now));
        let q = query(
            "classifier_eval",
            Some(filter),
            vec![("expires_at", "ASCENDING")],
            Some(limit),
            None,
        );
        let docs = self
            .repo
            .fs
            .run_query("classifier_eval", q)
            .await
            .map_err(map_err)?;
        docs.iter()
            .map(|d| {
                let v = from_fields(&d.fields).map_err(|_| StoreError::Corrupt("fields"))?;
                let r: ClassifierEvalRecord =
                    serde_json::from_value(v).map_err(|_| StoreError::Corrupt("record"))?;
                Ok(r.eval_id)
            })
            .collect()
    }
}

// ---------- bakeoff_snapshots ----------

pub struct BakeoffSnapshotsImpl {
    repo: FsRepo<BakeoffSnapshotRecord>,
}
impl BakeoffSnapshotsImpl {
    pub fn new(fs: Arc<Firestore>) -> Self {
        Self {
            repo: FsRepo::new(fs, "bakeoff_snapshots"),
        }
    }
}
#[async_trait]
impl Repo<SnapshotId, BakeoffSnapshotRecord> for BakeoffSnapshotsImpl {
    async fn get(
        &self,
        key: &SnapshotId,
    ) -> Result<Option<Versioned<BakeoffSnapshotRecord>>, StoreError> {
        self.repo.get(key).await
    }
    async fn put(
        &self,
        record: &BakeoffSnapshotRecord,
        pre: Precondition,
    ) -> Result<Version, StoreError> {
        self.repo.put(record, pre).await
    }
    async fn delete(&self, key: &SnapshotId, pre: Precondition) -> Result<(), StoreError> {
        self.repo.delete(key, pre).await
    }
}
#[async_trait]
impl BakeoffSnapshotRepo for BakeoffSnapshotsImpl {
    async fn list(
        &self,
        page: PageRequest,
    ) -> Result<Page<Versioned<BakeoffSnapshotRecord>>, StoreError> {
        run_paged(
            &self.repo.fs,
            "bakeoff_snapshots",
            None,
            vec![("created_at", "DESCENDING")],
            page,
            |r: &BakeoffSnapshotRecord| timestamp_value(r.created_at),
        )
        .await
    }
    async fn count(&self) -> Result<u64, StoreError> {
        let q = query("bakeoff_snapshots", None, vec![], None, None);
        let docs = self
            .repo
            .fs
            .run_query("bakeoff_snapshots", q)
            .await
            .map_err(map_err)?;
        Ok(docs.len() as u64)
    }
}

// ---------- config ----------

pub struct ConfigImpl {
    fs: Arc<Firestore>,
}
impl ConfigImpl {
    pub fn new(fs: Arc<Firestore>) -> Self {
        Self { fs }
    }
    fn doc_name(&self) -> String {
        self.fs.doc_name("config", "classifiers")
    }
}
#[async_trait]
impl ConfigRepo for ConfigImpl {
    async fn get_classifiers(&self) -> Result<Option<Versioned<ClassifiersConfig>>, StoreError> {
        match self.fs.get(&self.doc_name()).await {
            Ok(Some(doc)) => Ok(Some(decode_doc::<ClassifiersConfig>(&doc)?)),
            Ok(None) => Ok(None),
            Err(e) => Err(map_err(e)),
        }
    }
    async fn put_classifiers(
        &self,
        cfg: &ClassifiersConfig,
        pre: Precondition,
    ) -> Result<Version, StoreError> {
        let name = self.doc_name();
        let fields =
            to_fields(&serde_json::to_value(cfg).map_err(|_| StoreError::Invalid("serialise"))?)
                .map_err(|_| StoreError::Invalid("fields"))?;
        let write = Write {
            update: Some((name, fields)),
            current_document: precondition_json(&pre),
            ..Default::default()
        };
        let resp = self.fs.commit(&[write]).await.map_err(map_err)?;
        write_result_version(&resp, 0)
    }
}

// ---------- rate_limits ----------

pub struct RateLimitsImpl {
    fs: Arc<Firestore>,
}
impl RateLimitsImpl {
    pub fn new(fs: Arc<Firestore>) -> Self {
        Self { fs }
    }
    fn doc_name(&self, key: &str, window_start: OffsetDateTime) -> String {
        let id = format!("{key}:{}", window_start.unix_timestamp());
        self.fs.doc_name("rate_limits", &id)
    }
}
#[async_trait]
impl RateLimitRepo for RateLimitsImpl {
    async fn hit(
        &self,
        key: &RateLimitKey,
        window_start: OffsetDateTime,
        window: time::Duration,
    ) -> Result<u32, StoreError> {
        let name = self.doc_name(&key.0, window_start);
        let expires_at = window_start + window;
        let fields = json!({
            "key": { "stringValue": key.0 },
            "window_start_at": { "timestampValue": fmt_ts(window_start) },
            "expires_at": { "timestampValue": fmt_ts(expires_at) },
        });
        let write = Write {
            update: Some((name, fields.as_object().cloned().unwrap_or_default())),
            update_mask: Some(vec![
                "key".into(),
                "window_start_at".into(),
                "expires_at".into(),
            ]),
            update_transforms: Some(vec![json!({
                "fieldPath": "count",
                "increment": { "integerValue": "1" },
            })]),
            ..Default::default()
        };
        let resp = self.fs.commit(&[write]).await.map_err(map_err)?;
        let count = resp
            .pointer("/writeResults/0/transformResults/0/integerValue")
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<u32>().ok())
            .ok_or(StoreError::Corrupt("transform result"))?;
        Ok(count)
    }
    async fn expires_by(&self, now: OffsetDateTime, _limit: u32) -> Result<u64, StoreError> {
        let filter = cmp_filter("expires_at", "LESS_THAN_OR_EQUAL", timestamp_value(now));
        bulk_delete_by_filter(&self.fs, "rate_limits", filter).await
    }
}

// ---------- helpers ----------

fn hex(bytes: [u8; 32]) -> String {
    use std::fmt::Write as _;
    let mut s = String::with_capacity(64);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

fn fmt_ts(dt: OffsetDateTime) -> String {
    dt.format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_owned())
}

/// Query document names matching a filter, then delete them in batches of at
/// most 500 writes, repeating until the query returns nothing. Returns the
/// number deleted.
async fn bulk_delete_by_filter(
    fs: &Arc<Firestore>,
    collection: &str,
    filter: Value,
) -> Result<u64, StoreError> {
    let mut total = 0u64;
    loop {
        let q = query(collection, Some(filter.clone()), vec![], Some(500), None);
        let docs = fs.run_query(collection, q).await.map_err(map_err)?;
        if docs.is_empty() {
            break;
        }
        let writes: Vec<Write> = docs
            .iter()
            .map(|d| Write {
                delete: Some(d.name.clone()),
                ..Default::default()
            })
            .collect();
        fs.commit(&writes).await.map_err(map_err)?;
        total += writes.len() as u64;
    }
    Ok(total)
}
