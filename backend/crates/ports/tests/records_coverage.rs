//! Coverage for the record Debug/Keyed impls and `mailbox_id_for`.

use domain::{
    InviteStatus, JobId, JobMethod, JobStatus, MailboxId, MailboxStatus, MessageClass,
    NeedsAttentionReason, Provider, ProviderSubjectId, UserId,
};
use ports::keys::WrappedKey;
use ports::store::{
    mailbox_id_for, AgeBucket, AuthIntent, BakeoffSnapshotRecord, Ciphertext, ClassifierEvalRecord,
    ClassifiersConfig, EmailLookupHash, EvalHeaderFacts, EvalId, EvalOutcome, HeaderRulesResult,
    InviteId, InviteRecord, InviteRequestId, InviteRequestRecord, InviteRequestStatus, JobOutcome,
    JobOutcomeCode, JobRecord, Keyed, ListKeyHash, MailboxRecord, ModelErrorCode, ModelPrediction,
    NeedsAttentionId, NeedsAttentionRecord, PreAuthFields, RateLimitRecord, SessionHash,
    SessionRecord, SessionRecordId, SessionState, Sha256Hash, SnapshotId, SwipeDirection,
    TextTokensBucket, UserPseudoId, UserRecord,
};
use time::OffsetDateTime;
use uuid::Uuid;

fn t() -> OffsetDateTime {
    OffsetDateTime::UNIX_EPOCH
}
fn uid() -> UserId {
    UserId(Uuid::new_v4())
}
fn mid() -> MailboxId {
    MailboxId(Uuid::new_v4())
}
fn ct() -> Ciphertext {
    Ciphertext(vec![1, 2, 3])
}
fn h32() -> Sha256Hash {
    Sha256Hash([0xab; 32])
}
fn eh() -> EmailLookupHash {
    EmailLookupHash([0x01; 32])
}
fn lkh() -> ListKeyHash {
    ListKeyHash([0x02; 32])
}
fn sh() -> SessionHash {
    SessionHash([0x03; 32])
}

#[test]
fn user_record_keyed() {
    let r = UserRecord {
        user_id: uid(),
        created_at: t(),
        is_admin: false,
        wrapped_data_key: WrappedKey(vec![0]),
        experiments_consent_version: None,
        experiments_opted_in_at: None,
    };
    assert_eq!(r.key(), r.user_id);
}

#[test]
fn mailbox_record_debug_and_keyed() {
    let r = MailboxRecord {
        mailbox_id: mid(),
        user_id: uid(),
        provider: Provider::Gmail,
        provider_subject_id: ProviderSubjectId::new("subj")
            .unwrap_or_else(|_| panic!("subject id")),
        email_address: ct(),
        status: MailboxStatus::Connected,
        linked_at: t(),
        is_primary: true,
        refresh_token: None,
    };
    assert_eq!(r.key(), r.mailbox_id);
    assert!(format!("{r:?}").contains("MailboxRecord"));
}

#[test]
fn invite_record_debug_and_keyed() {
    let r = InviteRecord {
        invite_id: InviteId(Uuid::new_v4()),
        email_address: ct(),
        email_lookup: eh(),
        token_hash: h32(),
        status: InviteStatus::Pending,
        created_at: t(),
        last_sent_at: t(),
        expires_at: t(),
        purge_at: t(),
    };
    assert_eq!(r.key(), r.invite_id);
    assert!(format!("{r:?}").contains("InviteRecord"));
}

#[test]
fn invite_request_record_debug_and_keyed() {
    let r = InviteRequestRecord {
        request_id: InviteRequestId(Uuid::new_v4()),
        email_address: ct(),
        email_lookup: eh(),
        created_at: t(),
        status: InviteRequestStatus::Pending,
    };
    assert_eq!(r.key(), r.request_id);
    assert!(format!("{r:?}").contains("InviteRequestRecord"));
}

#[test]
fn job_record_keyed() {
    let r = JobRecord {
        job_id: JobId(Uuid::new_v4()),
        user_id: uid(),
        mailbox_id: mid(),
        list_key_hash: lkh(),
        method: JobMethod::OneClick,
        target: None,
        due_at: t(),
        status: JobStatus::Queued,
        attempts: 0,
        outcome: None,
        expires_at: t(),
    };
    assert_eq!(r.key(), r.job_id);
}

#[test]
fn needs_attention_record_debug_and_keyed() {
    let r = NeedsAttentionRecord {
        item_id: NeedsAttentionId(Uuid::new_v4()),
        user_id: uid(),
        mailbox_id: mid(),
        sender_display: ct(),
        link: None,
        reason_code: NeedsAttentionReason::HttpsOnlyUnsubscribe,
        created_at: t(),
        expires_at: t(),
    };
    assert_eq!(r.key(), r.item_id);
    assert!(format!("{r:?}").contains("NeedsAttentionRecord"));
}

#[test]
fn pre_auth_and_session_debug_and_keyed() {
    let pre = PreAuthFields {
        intent: AuthIntent::SignIn,
        oauth_state: ct(),
        nonce: ct(),
        pkce_verifier: ct(),
        invite_token_hash: None,
        pending_email: None,
        started_at: t(),
    };
    assert!(format!("{pre:?}").contains("PreAuthFields"));

    let r = SessionRecord {
        session_hash: sh(),
        session_record_id: SessionRecordId(Uuid::new_v4()),
        state: SessionState::PreAuth,
        user_id: None,
        csrf_token: "tok".into(),
        created_at: t(),
        last_seen_at: t(),
        recent_auth_at: None,
        expires_at: t(),
        pre_auth: Some(pre),
    };
    assert_eq!(r.key(), r.session_hash);
    assert!(format!("{r:?}").contains("SessionRecord"));
}

#[test]
fn classifier_eval_and_bakeoff_keyed() {
    let eval = ClassifierEvalRecord {
        eval_id: EvalId(Uuid::new_v4()),
        user_pseudo_id: UserPseudoId("p".into()),
        created_at: t(),
        expires_at: t(),
        header_rules: HeaderRulesResult {
            class: MessageClass::List,
            score: 5,
        },
        gemini: None,
        jev: None,
        outcome: EvalOutcome {
            direction: SwipeDirection::Right,
            undone: false,
            time_to_swipe_ms: 1,
        },
        header_facts: EvalHeaderFacts {
            has_list_unsubscribe: true,
            dkim_covers_list_unsubscribe: false,
            has_list_unsubscribe_post: false,
            has_list_id: false,
            has_feedback_id: false,
            precedence_bulk: false,
            auto_submitted: false,
            from_authenticated: false,
        },
        provider: Provider::Gmail,
        age_bucket: AgeBucket::Under7d,
        text_tokens_bucket: TextTokensBucket::Under100,
        lang_is_english: true,
        input_version: "v1".into(),
        question_version: "q1".into(),
        price_version: "p1".into(),
    };
    assert_eq!(eval.key(), eval.eval_id);

    let snap = BakeoffSnapshotRecord {
        snapshot_id: SnapshotId(Uuid::new_v4()),
        name: "n".into(),
        created_at: t(),
        labelled_swipes: 0,
        query_json: "{}".into(),
        report_json: "{}".into(),
    };
    assert_eq!(snap.key(), snap.snapshot_id);
}

#[test]
fn mailbox_id_for_is_deterministic() {
    let s = ProviderSubjectId::new("abc").unwrap_or_else(|_| panic!("subject id"));
    let a = mailbox_id_for(Provider::Gmail, &s);
    let b = mailbox_id_for(Provider::Gmail, &s);
    assert_eq!(a, b);
    let other = mailbox_id_for(
        Provider::Gmail,
        &ProviderSubjectId::new("xyz").unwrap_or_else(|_| panic!("subject id")),
    );
    assert_ne!(a, other);
}

#[test]
fn remaining_records_construct_and_debug() {
    let _ = ClassifiersConfig {
        gemini_enabled: true,
        jev_enabled: false,
        updated_at: t(),
    };
    let _ = RateLimitRecord {
        key: "k".into(),
        window_start: t(),
        count: 1,
        expires_at: t(),
    };
    let _ = ModelPrediction {
        class: None,
        score: None,
        confidence: None,
        probabilities: None,
        model_version: "m".into(),
        latency_ms: 0,
        input_tokens: None,
        error_code: Some(ModelErrorCode::Timeout),
    };
    let _ = JobOutcome {
        code: JobOutcomeCode::OneClickAccepted,
        at: t(),
    };
    let _ = TextTokensBucket::Over300;
    let _ = AgeBucket::Over5y;
}
