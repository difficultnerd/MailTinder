//! Tests for the domain types (T-101).

use std::fmt::Write as _;

use domain::{
    CategoryId, Classification, DomainError, HeaderFacts, LabelSet, Mailbox, MailboxIdentity,
    MailboxStatus, MailtoTarget, MessageClass, MessageId, MessageMeta, Provider, ProviderSubjectId,
    SenderKey, SwipeAction, Tunables, UnsubscribeOptions, UserId,
};
use proptest::prelude::*;
use time::OffsetDateTime;
use url::Url;
use uuid::Uuid;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn user() -> UserId {
    UserId(Uuid::new_v4())
}

fn mailbox_id() -> domain::MailboxId {
    domain::MailboxId(Uuid::new_v4())
}

fn identity() -> Result<MailboxIdentity, DomainError> {
    Ok(MailboxIdentity {
        provider: Provider::Gmail,
        subject: ProviderSubjectId::new("sub-123")?,
    })
}

#[test]
fn au_03_ac7_identity_equal_by_provider_and_subject() -> TestResult {
    let a = identity()?;
    let b = MailboxIdentity {
        provider: Provider::Gmail,
        subject: ProviderSubjectId::new("sub-123")?,
    };
    assert_eq!(a, b);
    let c = MailboxIdentity {
        provider: Provider::Gmail,
        subject: ProviderSubjectId::new("sub-456")?,
    };
    assert_ne!(a, c);
    Ok(())
}

#[test]
fn au_03_ac7_identity_has_no_email_field() -> TestResult {
    // Compiles only if MailboxIdentity has exactly provider + subject and no email field.
    let _ = MailboxIdentity {
        provider: Provider::Gmail,
        subject: ProviderSubjectId::new("sub-123")?,
    };
    Ok(())
}

#[test]
fn inv_3_mailbox_owned_by_one_user_only() -> TestResult {
    let owner = user();
    let other = user();
    let mb = Mailbox {
        id: mailbox_id(),
        user: owner,
        identity: identity()?,
        status: MailboxStatus::Connected,
        linked_at: OffsetDateTime::UNIX_EPOCH,
        is_primary: true,
    };
    assert!(mb.owned_by(&owner));
    assert!(!mb.owned_by(&other));
    Ok(())
}

#[test]
fn sr_01_ac1a_icloud_relay_unwrapped_to_original_sender() {
    let key = SenderKey::from_address("news_at_shop_example_com_k3j9x2@icloud.com");
    assert_eq!(key.as_str(), "news@shop.example.com");
}

#[test]
fn sr_01_ac1a_relay_with_bad_suffix_kept_as_is() {
    // Suffix too short (not 4+ alphanumerics).
    let key = SenderKey::from_address("news_at_shop_example_com_ab@icloud.com");
    assert_eq!(key.as_str(), "news_at_shop_example_com_ab@icloud.com");
    // Suffix has a non-alphanumeric.
    let key = SenderKey::from_address("news_at_shop_example_com_ab-cd@icloud.com");
    assert_eq!(key.as_str(), "news_at_shop_example_com_ab-cd@icloud.com");
}

#[test]
fn sr_01_ac1a_non_relay_domain_untouched() {
    let key = SenderKey::from_address("News@Example.com");
    assert_eq!(key.as_str(), "news@example.com");
}

proptest! {
    #[test]
    fn sr_01_ac1a_sender_key_lower_cased_and_trimmed(
        local in "[a-zA-Z0-9._-]{1,20}",
        domain in "[a-zA-Z0-9.-]{1,30}",
    ) {
        let input = format!("{local}@{domain}");
        let key = SenderKey::from_address(&input);
        let expected = input.to_lowercase();
        prop_assert_eq!(key.as_str(), &expected);
        prop_assert_eq!(SenderKey::from_address(key.as_str()), key);
    }
}

#[test]
fn xc_01_debug_redacts_sender_subject_and_ids() -> TestResult {
    let meta = MessageMeta {
        mailbox: mailbox_id(),
        id: MessageId::new("CANARY-T101-msgid")?,
        internal_date: OffsetDateTime::UNIX_EPOCH,
        from_display: "CANARY-T101-display".to_owned(),
        from_address: "CANARY-T101-addr@example.com".to_owned(),
        sender: SenderKey::from_address("CANARY-T101-sender@example.com"),
        subject: "CANARY-T101-subject".to_owned(),
        labels: LabelSet::new(),
        facts: HeaderFacts::default(),
    };
    let mut out = String::new();
    write!(&mut out, "{meta:?}")?;
    assert!(!out.contains("CANARY-T101"));
    assert!(out.contains("[redacted]"));
    Ok(())
}

#[test]
fn xc_01_debug_of_unsubscribe_options_shows_presence_only() -> TestResult {
    let opts = UnsubscribeOptions {
        one_click_https: Some(Url::parse("https://example.com/unsub")?),
        https: None,
        mailto: None,
    };
    let mut out = String::new();
    write!(&mut out, "{opts:?}")?;
    assert!(out.contains("one_click: true"));
    assert!(out.contains("https: false"));
    assert!(out.contains("mailto: false"));
    assert!(!out.contains("example.com"));
    Ok(())
}

#[test]
fn message_id_rejects_empty_long_and_control() {
    assert!(MessageId::new("").is_err());
    assert!(MessageId::new("a".repeat(257)).is_err());
    assert!(MessageId::new("has\ncontrol").is_err());
    assert!(MessageId::new("ok-id-123").is_ok());
}

#[test]
fn mailto_target_rejects_cr_lf_and_bad_address() {
    assert!(MailtoTarget::new("", None, None).is_err());
    assert!(MailtoTarget::new("no-at-sign", None, None).is_err());
    assert!(MailtoTarget::new("a@b@c", None, None).is_err());
    assert!(MailtoTarget::new("a@b", Some("line\nbreak"), None).is_err());
    assert!(MailtoTarget::new("a@b", None, Some("body\r\nx")).is_err());
    assert!(MailtoTarget::new("a@b", None, None).is_ok());
}

#[test]
fn serde_round_trip_ids_actions_and_classes() -> TestResult {
    let file = SwipeAction::File {
        category: CategoryId(Uuid::new_v4()),
    };
    let json = serde_json::to_string(&file)?;
    assert!(json.starts_with(r#"{"action":"file","category":""#));
    let back: SwipeAction = serde_json::from_str(&json)?;
    assert_eq!(file, back);

    let class = MessageClass::BulkNoHeader;
    let json = serde_json::to_string(&class)?;
    assert_eq!(json, r#""bulk_no_header""#);
    let back: MessageClass = serde_json::from_str(&json)?;
    assert_eq!(class, back);

    let id = MessageId::new("m-1")?;
    let json = serde_json::to_string(&id)?;
    let back: MessageId = serde_json::from_str(&json)?;
    assert_eq!(id, back);
    Ok(())
}

#[test]
fn tunables_default_matches_s2_glossary() {
    let t = Tunables::default();
    assert_eq!(t.unsub_delay, std::time::Duration::from_secs(5 * 60));
    assert_eq!(t.personal_block_threshold, 3);
    assert_eq!(t.skip_max_returns, 2);
    assert_eq!(t.preview_max_chars, 300);
    assert_eq!(t.keep_learning_threshold, 5);
    assert_eq!(t.filing_learned_threshold, 3);
    assert_eq!(t.boss_min_seen, 20);
    assert_eq!(t.boss_top_n, 5);
    assert_eq!(t.round_swipes, 50);
    assert_eq!(
        t.invite_ttl,
        std::time::Duration::from_secs(7 * 24 * 60 * 60)
    );
}

#[test]
fn classification_uses_partial_eq() {
    let a = Classification {
        class: MessageClass::List,
        bulk_score: 80,
        bulk_reason: "has unsubscribe link".to_owned(),
        confidence: Some(0.9),
        probabilities: None,
    };
    let b = a.clone();
    assert_eq!(a, b);
}
