//! Integration tests for `svc_common::mint` (T-503): refresh-token sealing and
//! opening, and on-demand access-token minting, against the deterministic fakes.
//!
//! Every test returns `Result`; an assertion failure is the test failing.
#![allow(
    clippy::pedantic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::missing_panics_doc,
    clippy::missing_errors_doc,
    clippy::too_many_lines
)]

use domain::{MailboxId, MailboxStatus, Provider, ProviderSubjectId, UserId};
use obs::Sensitive;
use ports::{
    Ciphertext, Clock, IdError, IdentityProvider, KeyService, MailboxRecord, Ports, Precondition,
    Rng, ServerStore, UserRecord,
};
use svc_common::mint::{mint_access_token, open_refresh_token, seal_refresh_token, MintError};
use testkit::Fakes;

/// A canary refresh token: it must never appear in a log, a record or a dump.
const CANARY_REFRESH: &str = "CANARY-refresh-token-0001";
/// A canary access token: minting-side proof that no access token is stored.
const CANARY_ACCESS: &str = "CANARY-access-token-0001";

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn missing(what: &str) -> Box<dyn std::error::Error> {
    format!("missing {what}").into()
}

async fn seed_user(fakes: &Fakes) -> Result<UserId, Box<dyn std::error::Error>> {
    let user = UserId::new(fakes.rng.uuid_v4());
    let wrapped = fakes.keys.new_user_key(&user).await?;
    fakes
        .store
        .users()
        .put(
            &UserRecord {
                user_id: user,
                created_at: fakes.clock.now(),
                is_admin: false,
                wrapped_data_key: wrapped,
                experiments_consent_version: None,
                experiments_opted_in_at: None,
            },
            Precondition::MustNotExist,
        )
        .await?;
    Ok(user)
}

async fn user_record(
    fakes: &Fakes,
    user: &UserId,
) -> Result<UserRecord, Box<dyn std::error::Error>> {
    Ok(fakes
        .store
        .users()
        .get(user)
        .await?
        .ok_or_else(|| missing("user"))?
        .record)
}

fn mailbox_owned_by(
    fakes: &Fakes,
    user: &UserId,
    status: MailboxStatus,
    refresh_token: Option<Ciphertext>,
) -> Result<MailboxRecord, Box<dyn std::error::Error>> {
    Ok(MailboxRecord {
        mailbox_id: MailboxId(fakes.rng.uuid_v4()),
        user_id: *user,
        provider: Provider::Gmail,
        provider_subject_id: ProviderSubjectId::new("sub-canary-0001")?,
        email_address: Ciphertext(vec![7u8; 24]),
        status,
        linked_at: fakes.clock.now(),
        is_primary: true,
        refresh_token,
    })
}

/// A stored mailbox record whose refresh token is sealed under the user's live
/// `data_key`.
async fn seed_connected(
    fakes: &Fakes,
    ports: &Ports,
    user: &UserId,
    token: &str,
) -> Result<MailboxRecord, Box<dyn std::error::Error>> {
    let record = mailbox_owned_by(fakes, user, MailboxStatus::Connected, None)?;
    let sealed = seal_refresh_token(
        ports,
        &user_record(fakes, user).await?,
        &record.mailbox_id,
        &Sensitive::new(token.to_owned()),
    )
    .await?;
    let record = MailboxRecord {
        refresh_token: Some(sealed),
        ..record
    };
    fakes
        .store
        .mailboxes()
        .put(&record, Precondition::MustNotExist)
        .await?;
    Ok(record)
}

fn dump(fakes: &Fakes) -> Result<String, Box<dyn std::error::Error>> {
    Ok(serde_json::to_string(&fakes.store.export_json())?)
}

#[tokio::test]
async fn un_01_ac4_mint_returns_fresh_token_and_stores_nothing() -> TestResult {
    let (ports, fakes) = testkit::fake_ports();
    let user = seed_user(&fakes).await?;
    let record = seed_connected(&fakes, &ports, &user, CANARY_REFRESH).await?;
    fakes
        .identity
        .script_refresh(CANARY_REFRESH, Ok(CANARY_ACCESS.to_owned()));

    let ctx = mint_access_token(&ports, &user, &record.mailbox_id).await?;
    assert_eq!(ctx.mailbox, record.mailbox_id);
    assert_eq!(ctx.access_token.expose().as_str(), CANARY_ACCESS);

    let stored = dump(&fakes)?;
    assert!(
        !stored.contains(CANARY_ACCESS),
        "the access token must not be written to any record"
    );
    Ok(())
}

#[tokio::test]
async fn un_01_ac6_invalid_grant_sets_needs_sign_in() -> TestResult {
    let (ports, fakes) = testkit::fake_ports();
    let user = seed_user(&fakes).await?;
    let record = seed_connected(&fakes, &ports, &user, CANARY_REFRESH).await?;
    fakes
        .identity
        .script_refresh(CANARY_REFRESH, Err(IdError::InvalidGrant));

    let result = mint_access_token(&ports, &user, &record.mailbox_id).await;
    assert!(matches!(result, Err(MintError::Revoked)));

    let after = fakes
        .store
        .mailboxes()
        .get(&record.mailbox_id)
        .await?
        .ok_or_else(|| missing("mailbox"))?;
    assert_eq!(after.record.status, MailboxStatus::NeedsSignIn);
    // The refresh token is kept for reconnect.
    assert!(after.record.refresh_token.is_some());
    Ok(())
}

#[tokio::test]
async fn st_03_ac1_status_needs_sign_in_after_revocation() -> TestResult {
    let (ports, fakes) = testkit::fake_ports();
    let user = seed_user(&fakes).await?;
    let record = seed_connected(&fakes, &ports, &user, CANARY_REFRESH).await?;
    // The user revoked the app in their Google account: the grant is gone.
    fakes
        .identity
        .revoke(&Sensitive::new(CANARY_REFRESH.to_owned()))
        .await?;

    let result = mint_access_token(&ports, &user, &record.mailbox_id).await;
    assert!(matches!(result, Err(MintError::Revoked)));

    let after = fakes
        .store
        .mailboxes()
        .get(&record.mailbox_id)
        .await?
        .ok_or_else(|| missing("mailbox"))?;
    assert_eq!(after.record.status, MailboxStatus::NeedsSignIn);
    Ok(())
}

#[tokio::test]
async fn un_01_ac6_transient_failure_does_not_set_needs_sign_in() -> TestResult {
    let (ports, fakes) = testkit::fake_ports();
    let user = seed_user(&fakes).await?;
    let record = seed_connected(&fakes, &ports, &user, CANARY_REFRESH).await?;
    fakes
        .identity
        .script_refresh(CANARY_REFRESH, Err(IdError::Unavailable));

    let result = mint_access_token(&ports, &user, &record.mailbox_id).await;
    assert!(matches!(result, Err(MintError::Transient)));

    let after = fakes
        .store
        .mailboxes()
        .get(&record.mailbox_id)
        .await?
        .ok_or_else(|| missing("mailbox"))?;
    assert_eq!(after.record.status, MailboxStatus::Connected);
    Ok(())
}

#[tokio::test]
async fn inv_3_mint_for_other_users_mailbox_is_missing() -> TestResult {
    let (ports, fakes) = testkit::fake_ports();
    let owner = seed_user(&fakes).await?;
    let other = seed_user(&fakes).await?;
    let record = seed_connected(&fakes, &ports, &owner, CANARY_REFRESH).await?;
    fakes
        .identity
        .script_refresh(CANARY_REFRESH, Ok(CANARY_ACCESS.to_owned()));

    let result = mint_access_token(&ports, &other, &record.mailbox_id).await;
    assert!(matches!(result, Err(MintError::MailboxMissing)));
    Ok(())
}

#[tokio::test]
async fn job_1_store_never_holds_access_token() -> TestResult {
    let (ports, fakes) = testkit::fake_ports();
    let user = seed_user(&fakes).await?;
    let first = seed_connected(&fakes, &ports, &user, CANARY_REFRESH).await?;
    let second = seed_connected(&fakes, &ports, &user, "CANARY-refresh-token-0002").await?;
    fakes
        .identity
        .script_refresh(CANARY_REFRESH, Ok(CANARY_ACCESS.to_owned()));

    let ctx = mint_access_token(&ports, &user, &first.mailbox_id).await?;
    assert_eq!(ctx.access_token.expose().as_str(), CANARY_ACCESS);
    // A second store (seal + write) after the mint must still hold no token.
    let sealed = seal_refresh_token(
        &ports,
        &user_record(&fakes, &user).await?,
        &second.mailbox_id,
        &Sensitive::new("CANARY-refresh-token-0003".to_owned()),
    )
    .await?;
    let updated = MailboxRecord {
        refresh_token: Some(sealed),
        ..second
    };
    fakes
        .store
        .mailboxes()
        .put(&updated, Precondition::None)
        .await?;

    let stored = dump(&fakes)?;
    assert!(
        !stored.contains(CANARY_ACCESS),
        "scanning every record must find no access token"
    );
    Ok(())
}

#[tokio::test]
async fn del_2_ciphertext_unusable_without_the_users_key() -> TestResult {
    let (ports, fakes) = testkit::fake_ports();
    let user = seed_user(&fakes).await?;
    let record = seed_connected(&fakes, &ports, &user, CANARY_REFRESH).await?;
    // The copy an attacker holds, and a user record whose data key the client
    // rotated: the old ciphertext can no longer be opened.
    let mut rotated = user_record(&fakes, &user).await?;
    rotated.wrapped_data_key = fakes.keys.new_user_key(&user).await?;

    let result = open_refresh_token(&ports, &rotated, &record).await;
    assert!(matches!(result, Err(MintError::Crypto)));
    // The original key still opens it, so the failure is the key, not the AAD.
    let ok = open_refresh_token(&ports, &user_record(&fakes, &user).await?, &record).await?;
    assert_eq!(ok.expose().as_str(), CANARY_REFRESH);
    Ok(())
}

#[tokio::test]
async fn asvs_v7_6_1_revoked_grant_needs_sign_in_and_logged() -> TestResult {
    let (ports, fakes) = testkit::fake_ports();
    let user = seed_user(&fakes).await?;
    let record = seed_connected(&fakes, &ports, &user, CANARY_REFRESH).await?;
    let (capture, _guard) =
        obs::capture("svc-common", obs::arc(obs::FixedClock(fakes.clock.now())));
    fakes
        .identity
        .script_refresh(CANARY_REFRESH, Err(IdError::InvalidGrant));

    let result = mint_access_token(&ports, &user, &record.mailbox_id).await;
    assert!(matches!(result, Err(MintError::Revoked)));

    let after = fakes
        .store
        .mailboxes()
        .get(&record.mailbox_id)
        .await?
        .ok_or_else(|| missing("mailbox"))?;
    assert_eq!(after.record.status, MailboxStatus::NeedsSignIn);

    let text = capture.text();
    assert!(text.contains("security"), "capture: {text}");
    assert!(text.contains("sign_in_required"), "capture: {text}");
    assert!(text.contains("token_invalid"), "capture: {text}");
    Ok(())
}

#[tokio::test]
async fn asvs_v10_1_1_refresh_token_never_in_log_or_record_clear() -> TestResult {
    let (ports, fakes) = testkit::fake_ports();
    let user = seed_user(&fakes).await?;
    let record = seed_connected(&fakes, &ports, &user, CANARY_REFRESH).await?;
    let (capture, _guard) =
        obs::capture("svc-common", obs::arc(obs::FixedClock(fakes.clock.now())));
    // A successful mint logs nothing; a revoked grant logs a security event.
    fakes
        .identity
        .script_refresh(CANARY_REFRESH, Ok(CANARY_ACCESS.to_owned()));
    let _ = mint_access_token(&ports, &user, &record.mailbox_id).await?;
    fakes
        .identity
        .script_refresh(CANARY_REFRESH, Err(IdError::InvalidGrant));
    let _ = mint_access_token(&ports, &user, &record.mailbox_id).await;

    let text = capture.text();
    assert!(!text.is_empty(), "no log lines were captured");
    let stored = dump(&fakes)?;
    assert!(
        !stored.contains(CANARY_REFRESH) && !stored.contains(CANARY_ACCESS),
        "no token may appear in clear in any record"
    );
    let needles = vec![CANARY_REFRESH.to_owned(), CANARY_ACCESS.to_owned()];
    assert!(
        obs::scan_for_leaks(&text, &needles).is_empty(),
        "a token leaked to the log"
    );
    Ok(())
}

#[tokio::test]
async fn asvs_v11_3_3_refresh_token_bound_to_mailbox_and_user() -> TestResult {
    let (ports, fakes) = testkit::fake_ports();
    let user = seed_user(&fakes).await?;
    let other = seed_user(&fakes).await?;
    let source = seed_connected(&fakes, &ports, &user, CANARY_REFRESH).await?;
    let ciphertext = source
        .refresh_token
        .clone()
        .ok_or_else(|| missing("refresh token"))?;

    // The same ciphertext on another mailbox of the same user fails to open:
    // the associated data binds it to one mailbox.
    let moved = mailbox_owned_by(
        &fakes,
        &user,
        MailboxStatus::Connected,
        Some(ciphertext.clone()),
    )?;
    let result = open_refresh_token(&ports, &user_record(&fakes, &user).await?, &moved).await;
    assert!(matches!(result, Err(MintError::Crypto)));

    // And on another user's mailbox it fails too.
    let foreign = mailbox_owned_by(&fakes, &other, MailboxStatus::Connected, Some(ciphertext))?;
    let result = open_refresh_token(&ports, &user_record(&fakes, &other).await?, &foreign).await;
    assert!(matches!(result, Err(MintError::Crypto)));
    Ok(())
}
