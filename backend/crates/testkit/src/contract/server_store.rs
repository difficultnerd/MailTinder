//! The shared `ServerStore` contract suite. Any `ServerStore` implementation
//! (the in-memory fake, the Firestore adapter) must pass it.
#![allow(
    clippy::many_single_char_names,
    clippy::wildcard_imports,
    clippy::must_use_candidate,
    clippy::missing_panics_doc,
    clippy::too_many_lines
)]

use std::sync::Arc;

use domain::{
    InviteStatus, JobId, JobMethod, JobStatus, MailboxId, MailboxStatus, MessageClass,
    NeedsAttentionReason, Provider, ProviderSubjectId, UserId,
};
use ports::store::{
    BakeoffSnapshotRecord, ClassifierEvalRecord, ClassifiersConfig, EmailLookupHash, EvalId,
    EvalOutcome, HeaderRulesResult, InviteId, InviteRecord, InviteRequestId, InviteRequestRecord,
    InviteRequestStatus, JobOutcome, JobOutcomeCode, JobRecord, ListKeyHash, MailboxRecord,
    NeedsAttentionId, NeedsAttentionRecord, PageRequest, Precondition, RateLimitKey, ServerStore,
    SessionHash, SessionRecord, SessionRecordId, SessionState, Sha256Hash, SnapshotId,
    SwipeDirection, UserPseudoId, UserRecord, Version,
};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

/// Sample records for tests (fixed UUIDs and times; example.com nowhere
/// because fields are ciphertext).
pub mod samples {
    use super::*;

    fn now() -> OffsetDateTime {
        OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap_or_else(|_| panic!("valid"))
    }
    fn user_id(n: u8) -> UserId {
        UserId(Uuid::from_u128(u128::from(n)))
    }
    fn mailbox_id(n: u8) -> MailboxId {
        MailboxId(Uuid::from_u128(0x1000 + u128::from(n)))
    }
    fn ct() -> ports::store::Ciphertext {
        ports::store::Ciphertext(vec![1, 2, 3, 4])
    }
    fn hash(n: u8) -> Sha256Hash {
        Sha256Hash([n; 32])
    }
    fn email_hash(n: u8) -> EmailLookupHash {
        EmailLookupHash([n; 32])
    }
    fn list_hash(n: u8) -> ListKeyHash {
        ListKeyHash([n; 32])
    }

    pub fn user(n: u8) -> UserRecord {
        UserRecord {
            user_id: user_id(n),
            created_at: now(),
            is_admin: false,
            wrapped_data_key: ports::WrappedKey(vec![n; 4]),
            experiments_consent_version: None,
            experiments_opted_in_at: None,
        }
    }

    pub fn mailbox(user: &UserId, n: u8) -> MailboxRecord {
        MailboxRecord {
            mailbox_id: mailbox_id(n),
            user_id: *user,
            provider: Provider::Gmail,
            provider_subject_id: ProviderSubjectId::new(format!("sub-{n}"))
                .unwrap_or_else(|_| panic!("valid")),
            email_address: ct(),
            status: MailboxStatus::Connected,
            linked_at: now(),
            is_primary: n == 0,
            refresh_token: Some(ct()),
        }
    }

    pub fn invite(n: u8) -> InviteRecord {
        InviteRecord {
            invite_id: InviteId(Uuid::from_u128(0x2000 + u128::from(n))),
            email_address: ct(),
            email_lookup: email_hash(n),
            token_hash: hash(n),
            status: InviteStatus::Pending,
            created_at: now(),
            last_sent_at: now(),
            expires_at: now() + Duration::days(7),
            purge_at: now() + Duration::days(37),
        }
    }

    pub fn invite_request(n: u8) -> InviteRequestRecord {
        InviteRequestRecord {
            request_id: InviteRequestId(Uuid::from_u128(0x3000 + u128::from(n))),
            email_address: ct(),
            email_lookup: email_hash(n),
            created_at: now(),
            status: InviteRequestStatus::Pending,
        }
    }

    pub fn job(user: &UserId, mailbox: &MailboxId, n: u8) -> JobRecord {
        JobRecord {
            job_id: JobId(Uuid::from_u128(0x4000 + u128::from(n))),
            user_id: *user,
            mailbox_id: *mailbox,
            list_key_hash: list_hash(n),
            method: JobMethod::OneClick,
            target: Some(ct()),
            due_at: now(),
            status: JobStatus::Queued,
            attempts: 0,
            outcome: None,
            expires_at: now() + Duration::hours(1),
        }
    }

    pub fn needs_attention(user: &UserId, mailbox: &MailboxId, n: u8) -> NeedsAttentionRecord {
        NeedsAttentionRecord {
            item_id: NeedsAttentionId(Uuid::from_u128(0x5000 + u128::from(n))),
            user_id: *user,
            mailbox_id: *mailbox,
            sender_display: ct(),
            link: Some(ct()),
            reason_code: NeedsAttentionReason::HttpsOnlyUnsubscribe,
            created_at: now(),
            expires_at: now() + Duration::days(30),
        }
    }

    pub fn session(user: Option<&UserId>, n: u8) -> SessionRecord {
        SessionRecord {
            session_hash: SessionHash([n; 32]),
            session_record_id: SessionRecordId(Uuid::from_u128(0x6000 + u128::from(n))),
            state: SessionState::Authenticated,
            user_id: user.copied(),
            csrf_token: "csrf".to_owned(),
            created_at: now(),
            last_seen_at: now(),
            recent_auth_at: Some(now()),
            expires_at: now() + Duration::minutes(15),
            pre_auth: None,
        }
    }

    pub fn eval(pseudo: &str, n: u8) -> ClassifierEvalRecord {
        ClassifierEvalRecord {
            eval_id: EvalId(Uuid::from_u128(0x7000 + u128::from(n))),
            user_pseudo_id: UserPseudoId(pseudo.to_owned()),
            created_at: now(),
            expires_at: now() + Duration::days(180),
            header_rules: HeaderRulesResult {
                class: MessageClass::List,
                score: 80,
            },
            gemini: None,
            jev: None,
            outcome: EvalOutcome {
                direction: SwipeDirection::Left,
                undone: false,
                time_to_swipe_ms: 100,
            },
            header_facts: ports::store::EvalHeaderFacts {
                has_list_unsubscribe: true,
                dkim_covers_list_unsubscribe: true,
                has_list_unsubscribe_post: false,
                has_list_id: false,
                has_feedback_id: false,
                precedence_bulk: false,
                auto_submitted: false,
                from_authenticated: true,
            },
            provider: Provider::Gmail,
            age_bucket: ports::store::AgeBucket::Under7d,
            text_tokens_bucket: ports::store::TextTokensBucket::Under100,
            lang_is_english: true,
            input_version: "v1".to_owned(),
            question_version: "v1".to_owned(),
            price_version: "v1".to_owned(),
        }
    }

    pub fn snapshot(n: u8) -> BakeoffSnapshotRecord {
        BakeoffSnapshotRecord {
            snapshot_id: SnapshotId(Uuid::from_u128(0x8000 + u128::from(n))),
            name: format!("snap-{n}"),
            created_at: now(),
            labelled_swipes: 10,
            query_json: "{}".to_owned(),
            report_json: "{}".to_owned(),
        }
    }

    pub fn classifiers(gemini: bool, jev: bool) -> ClassifiersConfig {
        ClassifiersConfig {
            gemini_enabled: gemini,
            jev_enabled: jev,
            updated_at: now(),
        }
    }
}

/// Each call to `make` returns a fresh, empty store. Runs every case in order
/// on a fresh store, returning `Err("<case>: <what failed>")` on the first
/// failure.
///
/// # Errors
///
/// Returns `Err` with the name of the first failing case.
pub async fn server_store<F, Fut>(make: F) -> Result<(), String>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = Arc<dyn ServerStore>>,
{
    use samples::*;

    // put then get, versioning, precondition on stale version.
    {
        let s = make().await;
        let u = user(1);
        let v1 = s
            .users()
            .put(&u, Precondition::None)
            .await
            .map_err(|e| format!("put: {e:?}"))?;
        let got = s
            .users()
            .get(&u.user_id)
            .await
            .map_err(|e| format!("get: {e:?}"))?
            .ok_or("get returned none")?;
        if got.record != u {
            return Err("put/get mismatch".into());
        }
        if got.version != v1 {
            return Err("version mismatch".into());
        }
        // A put with the current version succeeds and changes the version.
        // (The record must differ, or Firestore — which only bumps updateTime
        // on a content change — would return the same version.)
        let mut u2 = u.clone();
        u2.is_admin = true;
        let v2 = s
            .users()
            .put(&u2, Precondition::Matches(v1.clone()))
            .await
            .map_err(|e| format!("current Matches: {e:?}"))?;
        if v2 == got.version {
            return Err("version did not change".into());
        }
        // A put with the now-stale version fails.
        if s.users().put(&u2, Precondition::Matches(v1)).await.is_ok() {
            return Err("stale Matches should fail".into());
        }
    }

    // MustNotExist / MustExist
    {
        let s = make().await;
        let u = user(1);
        s.users()
            .put(&u, Precondition::None)
            .await
            .map_err(|e| format!("put: {e:?}"))?;
        if s.users().put(&u, Precondition::MustNotExist).await.is_ok() {
            return Err("MustNotExist should fail".into());
        }
        if s.users()
            .put(&user(2), Precondition::MustExist)
            .await
            .is_ok()
        {
            return Err("MustExist on missing should fail".into());
        }
    }

    // delete missing with None is Ok; stale Matches fails and leaves record
    {
        let s = make().await;
        let u = user(1);
        s.users()
            .delete(&u.user_id, Precondition::None)
            .await
            .map_err(|e| format!("delete missing: {e:?}"))?;
        s.users()
            .put(&u, Precondition::None)
            .await
            .map_err(|e| format!("put: {e:?}"))?;
        if s.users()
            .delete(&u.user_id, Precondition::Matches(Version("stale".into())))
            .await
            .is_ok()
        {
            return Err("stale delete should fail".into());
        }
        if s.users()
            .get(&u.user_id)
            .await
            .map_err(|e| format!("get: {e:?}"))?
            .is_none()
        {
            return Err("record should remain".into());
        }
    }

    // conditional transition race: two writers, exactly one succeeds
    {
        let s = make().await;
        let u = user(1);
        let mb = mailbox(&u.user_id, 1);
        let job = job(&u.user_id, &mb.mailbox_id, 1);
        let v = s
            .jobs()
            .put(&job, Precondition::None)
            .await
            .map_err(|e| format!("put: {e:?}"))?;
        let mut a = job.clone();
        a.attempts = 1;
        let mut b = job.clone();
        b.attempts = 2;
        let ra = s.jobs().put(&a, Precondition::Matches(v.clone())).await;
        let rb = s.jobs().put(&b, Precondition::Matches(v)).await;
        if ra.is_ok() == rb.is_ok() {
            return Err("race: exactly one writer must succeed".into());
        }
    }

    // mailbox by_subject / by_user / delete_all_for_user
    {
        let s = make().await;
        let u1 = user(1);
        let u2 = user(2);
        let m1 = mailbox(&u1.user_id, 1);
        let m2 = mailbox(&u1.user_id, 2);
        let m3 = mailbox(&u2.user_id, 3);
        for m in [&m1, &m2, &m3] {
            s.mailboxes()
                .put(m, Precondition::None)
                .await
                .map_err(|e| format!("put mb: {e:?}"))?;
        }
        let by_subj = s
            .mailboxes()
            .by_subject(Provider::Gmail, &m1.provider_subject_id)
            .await
            .map_err(|e| format!("by_subject: {e:?}"))?;
        if by_subj.map(|v| v.record.mailbox_id) != Some(m1.mailbox_id) {
            return Err("by_subject wrong".into());
        }
        let by_user = s
            .mailboxes()
            .by_user(&u1.user_id)
            .await
            .map_err(|e| format!("by_user: {e:?}"))?;
        if by_user.len() != 2 {
            return Err("by_user count wrong".into());
        }
        let removed = s
            .mailboxes()
            .delete_all_for_user(&u1.user_id)
            .await
            .map_err(|e| format!("del: {e:?}"))?;
        if removed != 2 {
            return Err("delete_all_for_user count wrong".into());
        }
    }

    // invite queries
    {
        let s = make().await;
        let i1 = invite(1);
        let i2 = invite(2);
        s.invites()
            .put(&i1, Precondition::None)
            .await
            .map_err(|e| format!("put: {e:?}"))?;
        s.invites()
            .put(&i2, Precondition::None)
            .await
            .map_err(|e| format!("put: {e:?}"))?;
        let by_token = s
            .invites()
            .by_token_hash(&i1.token_hash)
            .await
            .map_err(|e| format!("by_token: {e:?}"))?;
        if by_token.map(|v| v.record.invite_id) != Some(i1.invite_id) {
            return Err("by_token_hash wrong".into());
        }
        let by_email = s
            .invites()
            .by_email_lookup(&i1.email_lookup)
            .await
            .map_err(|e| format!("by_email: {e:?}"))?;
        if by_email.len() != 1 {
            return Err("by_email_lookup wrong".into());
        }
        let list = s
            .invites()
            .list(
                Some(InviteStatus::Pending),
                PageRequest {
                    limit: 10,
                    after: None,
                },
            )
            .await
            .map_err(|e| format!("list: {e:?}"))?;
        if list.items.len() != 2 {
            return Err("list count wrong".into());
        }
    }

    // job queries
    {
        let s = make().await;
        let u = user(1);
        let mb = mailbox(&u.user_id, 1);
        let j1 = job(&u.user_id, &mb.mailbox_id, 1);
        let mut j2 = job(&u.user_id, &mb.mailbox_id, 2);
        j2.outcome = Some(JobOutcome {
            code: JobOutcomeCode::OneClickAccepted,
            at: OffsetDateTime::now_utc(),
        });
        s.jobs()
            .put(&j1, Precondition::None)
            .await
            .map_err(|e| format!("put: {e:?}"))?;
        s.jobs()
            .put(&j2, Precondition::None)
            .await
            .map_err(|e| format!("put: {e:?}"))?;
        let queued = s
            .jobs()
            .queued_for_list(&u.user_id, &j1.list_key_hash)
            .await
            .map_err(|e| format!("queued: {e:?}"))?;
        if queued.len() != 1 {
            return Err("queued_for_list wrong".into());
        }
        let with_outcome = s
            .jobs()
            .by_user_with_outcome(&u.user_id, 10)
            .await
            .map_err(|e| format!("outcome: {e:?}"))?;
        if with_outcome.len() != 1 {
            return Err("by_user_with_outcome wrong".into());
        }
    }

    // needs attention paging
    {
        let s = make().await;
        let u = user(1);
        let mb = mailbox(&u.user_id, 1);
        for n in 1..=5 {
            let na = needs_attention(&u.user_id, &mb.mailbox_id, n);
            s.needs_attention()
                .put(&na, Precondition::None)
                .await
                .map_err(|e| format!("put: {e:?}"))?;
        }
        let count = s
            .needs_attention()
            .count_for_user(&u.user_id)
            .await
            .map_err(|e| format!("count: {e:?}"))?;
        if count != 5 {
            return Err("count wrong".into());
        }
        let page1 = s
            .needs_attention()
            .by_user(
                &u.user_id,
                PageRequest {
                    limit: 2,
                    after: None,
                },
            )
            .await
            .map_err(|e| format!("page1: {e:?}"))?;
        if page1.items.len() != 2 || page1.next.is_none() {
            return Err("page1 wrong".into());
        }
        let page2 = s
            .needs_attention()
            .by_user(
                &u.user_id,
                PageRequest {
                    limit: 2,
                    after: page1.next,
                },
            )
            .await
            .map_err(|e| format!("page2: {e:?}"))?;
        if page2.items.len() != 2 {
            return Err("page2 wrong".into());
        }
    }

    // session queries
    {
        let s = make().await;
        let u = user(1);
        let s1 = session(Some(&u.user_id), 1);
        let s2 = session(Some(&u.user_id), 2);
        s.sessions()
            .put(&s1, Precondition::None)
            .await
            .map_err(|e| format!("put: {e:?}"))?;
        s.sessions()
            .put(&s2, Precondition::None)
            .await
            .map_err(|e| format!("put: {e:?}"))?;
        let by_user = s
            .sessions()
            .by_user(&u.user_id)
            .await
            .map_err(|e| format!("by_user: {e:?}"))?;
        if by_user.len() != 2 {
            return Err("session by_user wrong".into());
        }
        let removed = s
            .sessions()
            .delete_all_for_user(&u.user_id)
            .await
            .map_err(|e| format!("del: {e:?}"))?;
        if removed != 2 {
            return Err("session delete_all wrong".into());
        }
    }

    // classifier eval range + delete_for_users
    {
        let s = make().await;
        let e1 = eval("pseudo-a", 1);
        let e2 = eval("pseudo-b", 2);
        s.classifier_eval()
            .put(&e1, Precondition::None)
            .await
            .map_err(|e| format!("put: {e:?}"))?;
        s.classifier_eval()
            .put(&e2, Precondition::None)
            .await
            .map_err(|e| format!("put: {e:?}"))?;
        let range = s
            .classifier_eval()
            .range(
                e1.created_at,
                e1.created_at + Duration::days(1),
                PageRequest {
                    limit: 10,
                    after: None,
                },
            )
            .await
            .map_err(|e| format!("range: {e:?}"))?;
        if range.items.len() != 2 {
            return Err("range wrong".into());
        }
        let removed = s
            .classifier_eval()
            .delete_for_users(&[UserPseudoId("pseudo-a".into())])
            .await
            .map_err(|e| format!("del: {e:?}"))?;
        if removed != 1 {
            return Err("delete_for_users wrong".into());
        }
    }

    // snapshot list + count
    {
        let s = make().await;
        let s1 = snapshot(1);
        let s2 = snapshot(2);
        s.bakeoff_snapshots()
            .put(&s1, Precondition::None)
            .await
            .map_err(|e| format!("put: {e:?}"))?;
        s.bakeoff_snapshots()
            .put(&s2, Precondition::None)
            .await
            .map_err(|e| format!("put: {e:?}"))?;
        let count = s
            .bakeoff_snapshots()
            .count()
            .await
            .map_err(|e| format!("count: {e:?}"))?;
        if count != 2 {
            return Err("snapshot count wrong".into());
        }
    }

    // config
    {
        let s = make().await;
        if s.config()
            .get_classifiers()
            .await
            .map_err(|e| format!("get: {e:?}"))?
            .is_some()
        {
            return Err("config should start absent".into());
        }
        let cfg = classifiers(true, false);
        s.config()
            .put_classifiers(&cfg, Precondition::MustNotExist)
            .await
            .map_err(|e| format!("put: {e:?}"))?;
        let got = s
            .config()
            .get_classifiers()
            .await
            .map_err(|e| format!("get: {e:?}"))?;
        if got.map(|v| v.record.gemini_enabled) != Some(true) {
            return Err("config readback wrong".into());
        }
    }

    // rate limits
    {
        let s = make().await;
        let key = RateLimitKey("k".into());
        let ws = OffsetDateTime::now_utc();
        let w = Duration::minutes(1);
        let c1 = s
            .rate_limits()
            .hit(&key, ws, w)
            .await
            .map_err(|e| format!("hit: {e:?}"))?;
        let c2 = s
            .rate_limits()
            .hit(&key, ws, w)
            .await
            .map_err(|e| format!("hit: {e:?}"))?;
        let c3 = s
            .rate_limits()
            .hit(&key, ws, w)
            .await
            .map_err(|e| format!("hit: {e:?}"))?;
        if (c1, c2, c3) != (1, 2, 3) {
            return Err("rate limit counts wrong".into());
        }
        let ws2 = ws + Duration::minutes(2);
        let c4 = s
            .rate_limits()
            .hit(&key, ws2, w)
            .await
            .map_err(|e| format!("hit: {e:?}"))?;
        if c4 != 1 {
            return Err("new window should start at 1".into());
        }
    }

    // paging limit clamp
    {
        let s = make().await;
        let u = user(1);
        s.users()
            .put(&u, Precondition::None)
            .await
            .map_err(|e| format!("put: {e:?}"))?;
        let page = s
            .users()
            .list(PageRequest {
                limit: 0,
                after: None,
            })
            .await
            .map_err(|e| format!("list: {e:?}"))?;
        if page.items.len() != 1 {
            return Err("limit 0 should be treated as 1".into());
        }
    }

    Ok(())
}
