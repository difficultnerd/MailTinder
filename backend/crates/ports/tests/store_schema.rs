//! Schema and invariant tests for the server store records (T-201b).

use base64::Engine;
use domain::{
    InviteStatus, JobId, JobMethod, JobStatus, MailboxId, MailboxStatus, MessageClass,
    NeedsAttentionReason, Provider, ProviderSubjectId, UserId,
};
use ports::store::{
    mailbox_id_for, AgeBucket, BakeoffSnapshotRecord, Ciphertext, ClassifierEvalRecord,
    ClassifiersConfig, EmailLookupHash, EvalHeaderFacts, EvalId, EvalOutcome, HeaderRulesResult,
    InviteRecord, InviteRequestRecord, InviteRequestStatus, JobRecord, ListKeyHash, MailboxRecord,
    NeedsAttentionRecord, RateLimitRecord, SessionHash, SessionRecord, SessionRecordId,
    SessionState, Sha256Hash, SnapshotId, SwipeDirection, TextTokensBucket, UserPseudoId,
    UserRecord, COLLECTIONS,
};
use serde_json::Value;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

fn now() -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap_or_else(|_| panic!("valid"))
}

fn user() -> UserId {
    UserId(Uuid::new_v4())
}

fn mailbox_id() -> MailboxId {
    MailboxId(Uuid::new_v4())
}

fn subject() -> ProviderSubjectId {
    ProviderSubjectId::new("sub-1").unwrap_or_else(|_| panic!("valid"))
}

fn ciphertext() -> Ciphertext {
    Ciphertext(vec![1, 2, 3, 4])
}

fn hash32() -> Sha256Hash {
    Sha256Hash([7u8; 32])
}

fn email_hash() -> EmailLookupHash {
    EmailLookupHash([8u8; 32])
}

fn list_hash() -> ListKeyHash {
    ListKeyHash([9u8; 32])
}

fn session_hash() -> SessionHash {
    SessionHash([10u8; 32])
}

fn user_record() -> UserRecord {
    UserRecord {
        user_id: user(),
        created_at: now(),
        is_admin: false,
        wrapped_data_key: ports::WrappedKey(vec![1, 2, 3]),
        experiments_consent_version: None,
        experiments_opted_in_at: None,
    }
}

fn mailbox_record() -> MailboxRecord {
    MailboxRecord {
        mailbox_id: mailbox_id(),
        user_id: user(),
        provider: Provider::Gmail,
        provider_subject_id: subject(),
        email_address: ciphertext(),
        status: MailboxStatus::Connected,
        linked_at: now(),
        is_primary: true,
        refresh_token: Some(ciphertext()),
    }
}

fn invite_record() -> InviteRecord {
    InviteRecord {
        invite_id: ports::InviteId(Uuid::new_v4()),
        email_address: ciphertext(),
        email_lookup: email_hash(),
        token_hash: hash32(),
        status: InviteStatus::Pending,
        created_at: now(),
        last_sent_at: now(),
        expires_at: now() + Duration::days(7),
        purge_at: now() + Duration::days(37),
    }
}

fn invite_request_record() -> InviteRequestRecord {
    InviteRequestRecord {
        request_id: ports::InviteRequestId(Uuid::new_v4()),
        email_address: ciphertext(),
        email_lookup: email_hash(),
        created_at: now(),
        status: InviteRequestStatus::Pending,
    }
}

fn job_record() -> JobRecord {
    JobRecord {
        job_id: JobId(Uuid::new_v4()),
        user_id: user(),
        mailbox_id: mailbox_id(),
        list_key_hash: list_hash(),
        method: JobMethod::OneClick,
        target: Some(ciphertext()),
        due_at: now(),
        status: JobStatus::Queued,
        attempts: 0,
        outcome: None,
        expires_at: now() + Duration::hours(1),
    }
}

fn needs_attention_record() -> NeedsAttentionRecord {
    NeedsAttentionRecord {
        item_id: ports::NeedsAttentionId(Uuid::new_v4()),
        user_id: user(),
        mailbox_id: mailbox_id(),
        sender_display: ciphertext(),
        link: Some(ciphertext()),
        reason_code: NeedsAttentionReason::HttpsOnlyUnsubscribe,
        created_at: now(),
        expires_at: now() + Duration::days(30),
    }
}

fn session_record() -> SessionRecord {
    SessionRecord {
        session_hash: session_hash(),
        session_record_id: SessionRecordId(Uuid::new_v4()),
        state: SessionState::Authenticated,
        user_id: Some(user()),
        csrf_token: "csrf".to_owned(),
        created_at: now(),
        last_seen_at: now(),
        recent_auth_at: Some(now()),
        expires_at: now() + Duration::minutes(15),
        pre_auth: None,
    }
}

fn eval_record() -> ClassifierEvalRecord {
    ClassifierEvalRecord {
        eval_id: EvalId(Uuid::new_v4()),
        user_pseudo_id: UserPseudoId("abc123".to_owned()),
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
        header_facts: EvalHeaderFacts {
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
        age_bucket: AgeBucket::Under7d,
        text_tokens_bucket: TextTokensBucket::Under100,
        lang_is_english: true,
        input_version: "v1".to_owned(),
        question_version: "v1".to_owned(),
        price_version: "v1".to_owned(),
    }
}

fn snapshot_record() -> BakeoffSnapshotRecord {
    BakeoffSnapshotRecord {
        snapshot_id: SnapshotId(Uuid::new_v4()),
        name: "snap".to_owned(),
        created_at: now(),
        labelled_swipes: 10,
        query_json: "{}".to_owned(),
        report_json: "{}".to_owned(),
    }
}

fn config_record() -> ClassifiersConfig {
    ClassifiersConfig {
        gemini_enabled: true,
        jev_enabled: false,
        updated_at: now(),
    }
}

fn rate_limit_record() -> RateLimitRecord {
    RateLimitRecord {
        key: "k".to_owned(),
        window_start: now(),
        count: 1,
        expires_at: now() + Duration::minutes(1),
    }
}

/// Collect every key at every depth of a JSON value.
fn all_keys(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::Object(map) => {
            for (k, val) in map {
                out.push(k.clone());
                all_keys(val, out);
            }
        }
        Value::Array(items) => {
            for item in items {
                all_keys(item, out);
            }
        }
        _ => {}
    }
}

fn json(record: &impl serde::Serialize) -> Value {
    serde_json::to_value(record).unwrap_or_else(|_| panic!("serialise"))
}

#[test]
fn inv_1_no_record_has_body_subject_or_snippet_field() {
    let records: Vec<Value> = vec![
        json(&user_record()),
        json(&mailbox_record()),
        json(&invite_record()),
        json(&invite_request_record()),
        json(&job_record()),
        json(&needs_attention_record()),
        json(&session_record()),
        json(&eval_record()),
        json(&snapshot_record()),
        json(&config_record()),
        json(&rate_limit_record()),
    ];
    for record in records {
        let mut keys = Vec::new();
        all_keys(&record, &mut keys);
        for key in keys {
            // INV-1 forbids message content fields. `provider_subject_id` is a
            // Google `sub` identifier, not a message subject, so match the
            // exact field names rather than substrings.
            let lower = key.to_ascii_lowercase();
            assert!(
                lower != "body"
                    && lower != "subject"
                    && lower != "snippet"
                    && lower != "preview"
                    && !lower.contains("message_body")
                    && !lower.contains("message_subject")
                    && !lower.contains("mail_subject")
                    && !lower.contains("email_subject"),
                "record has forbidden field `{key}`"
            );
        }
    }
}

#[test]
fn inv_2_job_and_needs_attention_records_require_expires_at() {
    let mut job = json(&job_record());
    job.as_object_mut()
        .unwrap_or_else(|| panic!("object"))
        .remove("expires_at");
    assert!(serde_json::from_value::<JobRecord>(job).is_err());

    let mut na = json(&needs_attention_record());
    na.as_object_mut()
        .unwrap_or_else(|| panic!("object"))
        .remove("expires_at");
    assert!(serde_json::from_value::<NeedsAttentionRecord>(na).is_err());
}

#[test]
fn inv_3_mailbox_record_has_exactly_one_user() {
    let mut rec = json(&mailbox_record());
    rec.as_object_mut()
        .unwrap_or_else(|| panic!("object"))
        .remove("user_id");
    assert!(serde_json::from_value::<MailboxRecord>(rec).is_err());
}

#[test]
fn cfg_1_config_document_holds_only_allowed_fields() {
    let v = json(&config_record());
    let keys: Vec<String> = v
        .as_object()
        .unwrap_or_else(|| panic!("object"))
        .keys()
        .cloned()
        .collect();
    assert_eq!(keys.len(), 3);
    assert!(keys.contains(&"gemini_enabled".to_owned()));
    assert!(keys.contains(&"jev_enabled".to_owned()));
    assert!(keys.contains(&"updated_at".to_owned()));

    let mut extra = json(&config_record());
    extra
        .as_object_mut()
        .unwrap_or_else(|| panic!("object"))
        .insert("extra".to_owned(), Value::Bool(true));
    assert!(serde_json::from_value::<ClassifiersConfig>(extra).is_err());
}

#[test]
fn job_1_job_record_has_no_access_token_field() {
    let v = json(&job_record());
    let mut keys = Vec::new();
    all_keys(&v, &mut keys);
    for key in keys {
        assert!(
            !key.to_ascii_lowercase().contains("token"),
            "job record has a token field `{key}`"
        );
    }
}

#[test]
fn exp_1_eval_record_fields_match_allowed_schema() {
    let v = json(&eval_record());
    let mut keys = Vec::new();
    all_keys(&v, &mut keys);
    // No body/subject/snippet/preview anywhere.
    for key in &keys {
        let lower = key.to_ascii_lowercase();
        assert!(
            !lower.contains("body")
                && !lower.contains("subject")
                && !lower.contains("snippet")
                && !lower.contains("preview"),
            "eval record has forbidden field `{key}`"
        );
    }
}

#[test]
fn s5_every_collection_has_a_schema_entry() {
    // The eleven collections are exactly the ones with a record type.
    assert_eq!(COLLECTIONS.len(), 11);
    for c in COLLECTIONS {
        assert!(
            matches!(
                c,
                "users"
                    | "mailboxes"
                    | "invites"
                    | "invite_requests"
                    | "jobs"
                    | "needs_attention"
                    | "sessions"
                    | "classifier_eval"
                    | "bakeoff_snapshots"
                    | "config"
                    | "rate_limits"
            ),
            "unexpected collection {c}"
        );
    }
}

#[test]
fn mailbox_id_for_is_deterministic_and_provider_scoped() {
    let a = mailbox_id_for(Provider::Gmail, &subject());
    let b = mailbox_id_for(Provider::Gmail, &subject());
    assert_eq!(a, b);
    let other = mailbox_id_for(
        Provider::Gmail,
        &ProviderSubjectId::new("sub-2").unwrap_or_else(|_| panic!("valid")),
    );
    assert_ne!(a, other);
}

#[test]
fn ciphertext_and_hashes_round_trip_and_reject_bad_length() -> Result<(), Box<dyn std::error::Error>>
{
    let ct = Ciphertext(vec![1, 2, 3, 4]);
    let s = serde_json::to_string(&ct)?;
    let back: Ciphertext = serde_json::from_str(&s)?;
    assert_eq!(ct, back);

    let h = Sha256Hash([7u8; 32]);
    let s = serde_json::to_string(&h)?;
    assert_eq!(s.len(), 66); // quotes + 64 hex
    let back: Sha256Hash = serde_json::from_str(&s)?;
    assert_eq!(h, back);

    // Wrong length hex fails.
    assert!(serde_json::from_str::<Sha256Hash>("\"abcd\"").is_err());
    Ok(())
}

#[test]
fn sensitive_records_debug_prints_no_ciphertext() {
    let sample = vec![1, 2, 3, 4];
    let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&sample);
    let out = format!("{:?}", mailbox_record());
    assert!(!out.contains(&b64));
    let out = format!("{:?}", invite_record());
    assert!(!out.contains(&b64));
    let out = format!("{:?}", session_record());
    assert!(!out.contains(&b64));
}
