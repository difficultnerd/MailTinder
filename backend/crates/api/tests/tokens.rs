//! `TokenService` mapping, cache and storage tests (T-503).
#![allow(
    clippy::pedantic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::missing_panics_doc,
    clippy::missing_errors_doc,
    clippy::too_many_lines
)]

use std::sync::Arc;

use api::error::ApiError;
use api::state::AppState;
use api::tokens::{TokenService, ACCESS_TOKEN_CACHE_S};
use api::{app_state, config::ApiConfig};
use domain::{MailboxId, MailboxStatus, Provider, ProviderSubjectId, UserId};
use obs::Sensitive;
use ports::{
    Ciphertext, Clock, IdError, KeyService, MailboxRecord, Precondition, Rng, ServerStore,
    UserRecord,
};
use svc_common::mint::seal_refresh_token;
use testkit::Fakes;
use time::Duration;

const CANARY_REFRESH: &str = "CANARY-refresh-token-0001";

fn missing(what: &str) -> Box<dyn std::error::Error> {
    format!("missing {what}").into()
}

fn fixture() -> Result<(AppState, Fakes), Box<dyn std::error::Error>> {
    let (ports, fakes) = testkit::fake_ports();
    let config = ApiConfig::new(
        "https://mailtinder.test".to_owned(),
        "fake-client".to_owned(),
        Sensitive::new(b"fake-rate-key".to_vec()),
        Sensitive::new(b"fake-email-key".to_vec()),
    )?;
    Ok((app_state(Arc::new(ports), Arc::new(config)), fakes))
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

async fn seed_mailbox(
    fakes: &Fakes,
    user: &UserId,
    status: MailboxStatus,
) -> Result<MailboxRecord, Box<dyn std::error::Error>> {
    let record = MailboxRecord {
        mailbox_id: MailboxId(fakes.rng.uuid_v4()),
        user_id: *user,
        provider: Provider::Gmail,
        provider_subject_id: ProviderSubjectId::new("sub-canary-0001")?,
        email_address: Ciphertext(vec![7u8; 24]),
        status,
        linked_at: fakes.clock.now(),
        is_primary: true,
        refresh_token: None,
    };
    fakes
        .store
        .mailboxes()
        .put(&record, Precondition::MustNotExist)
        .await?;
    Ok(record)
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

/// A connected mailbox whose refresh token is sealed under the user's key.
async fn seed_connected(
    state: &AppState,
    fakes: &Fakes,
    user: &UserId,
    token: &str,
) -> Result<MailboxRecord, Box<dyn std::error::Error>> {
    let record = seed_mailbox(fakes, user, MailboxStatus::Connected).await?;
    let sealed = seal_refresh_token(
        &state.ports,
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
        .put(&record, Precondition::None)
        .await?;
    Ok(record)
}

#[tokio::test]
async fn api_mailbox_ctx_maps_errors() -> Result<(), Box<dyn std::error::Error>> {
    // A mailbox ID from a request body that is not the user's -> not found.
    let (state, fakes) = fixture()?;
    let user = seed_user(&fakes).await?;
    let absent = MailboxId(fakes.rng.uuid_v4());
    assert_eq!(
        state.tokens.mailbox_ctx(&state, &user, &absent).await.err(),
        Some(ApiError::NotFound)
    );

    // No stored grant -> needs_sign_in.
    let (state, fakes) = fixture()?;
    let user = seed_user(&fakes).await?;
    let record = seed_mailbox(&fakes, &user, MailboxStatus::Connected).await?;
    assert_eq!(
        state
            .tokens
            .mailbox_ctx(&state, &user, &record.mailbox_id)
            .await
            .err(),
        Some(ApiError::MailboxNeedsSignIn {
            mailbox_id: Some(record.mailbox_id.0)
        })
    );

    // Provider outage -> provider_unavailable, never needs_sign_in.
    let (state, fakes) = fixture()?;
    let user = seed_user(&fakes).await?;
    let record = seed_connected(&state, &fakes, &user, CANARY_REFRESH).await?;
    fakes
        .identity
        .script_refresh(CANARY_REFRESH, Err(IdError::Unavailable));
    assert_eq!(
        state
            .tokens
            .mailbox_ctx(&state, &user, &record.mailbox_id)
            .await
            .err(),
        Some(ApiError::ProviderUnavailable {
            mailbox_id: Some(record.mailbox_id.0),
            retry_after_s: None
        })
    );

    // A rotated data key -> crypto -> internal, with no detail leaked.
    let (state, fakes) = fixture()?;
    let user = seed_user(&fakes).await?;
    let record = seed_connected(&state, &fakes, &user, CANARY_REFRESH).await?;
    let mut rotated = user_record(&fakes, &user).await?;
    rotated.wrapped_data_key = fakes.keys.new_user_key(&user).await?;
    fakes
        .store
        .users()
        .put(&rotated, Precondition::None)
        .await?;
    assert_eq!(
        state
            .tokens
            .mailbox_ctx(&state, &user, &record.mailbox_id)
            .await
            .err(),
        Some(ApiError::Internal)
    );
    Ok(())
}

#[tokio::test]
async fn api_mailbox_ctx_cache_expires_after_300_s() -> Result<(), Box<dyn std::error::Error>> {
    let (state, fakes) = fixture()?;
    let user = seed_user(&fakes).await?;
    let record = seed_connected(&state, &fakes, &user, CANARY_REFRESH).await?;
    let service = Arc::new(TokenService::new());
    fakes
        .identity
        .script_refresh(CANARY_REFRESH, Ok("access-token-1".to_owned()));
    fakes
        .identity
        .script_refresh(CANARY_REFRESH, Ok("access-token-2".to_owned()));

    let first = service
        .mailbox_ctx(&state, &user, &record.mailbox_id)
        .await?;
    assert_eq!(first.access_token.expose().as_str(), "access-token-1");

    // Inside the window the cached token is reused, so the second scripted
    // response is not consumed.
    let second = service
        .mailbox_ctx(&state, &user, &record.mailbox_id)
        .await?;
    assert_eq!(second.access_token.expose().as_str(), "access-token-1");

    // Past the window the token is minted again.
    fakes
        .clock
        .advance(Duration::seconds(ACCESS_TOKEN_CACHE_S + 1));
    let third = service
        .mailbox_ctx(&state, &user, &record.mailbox_id)
        .await?;
    assert_eq!(third.access_token.expose().as_str(), "access-token-2");
    assert_eq!(third.mailbox, record.mailbox_id);
    Ok(())
}

#[tokio::test]
async fn api_store_refresh_token_sets_connected() -> Result<(), Box<dyn std::error::Error>> {
    let (state, fakes) = fixture()?;
    let user = seed_user(&fakes).await?;
    let record = seed_mailbox(&fakes, &user, MailboxStatus::NeedsSignIn).await?;

    state
        .tokens
        .store_refresh_token(
            &state,
            &user,
            &record.mailbox_id,
            Sensitive::new(CANARY_REFRESH.to_owned()),
        )
        .await?;

    let after = fakes
        .store
        .mailboxes()
        .get(&record.mailbox_id)
        .await?
        .ok_or_else(|| missing("mailbox"))?;
    assert_eq!(after.record.status, MailboxStatus::Connected);
    let ciphertext = after
        .record
        .refresh_token
        .clone()
        .ok_or_else(|| missing("sealed token"))?;
    assert_ne!(ciphertext.0, CANARY_REFRESH.as_bytes().to_vec());

    // The stored grant opens back to the token.
    let opened = svc_common::mint::open_refresh_token(
        &state.ports,
        &user_record(&fakes, &user).await?,
        &after.record,
    )
    .await?;
    assert_eq!(opened.expose().as_str(), CANARY_REFRESH);

    // Another user cannot store onto this mailbox (INV-3).
    let other = seed_user(&fakes).await?;
    assert_eq!(
        state
            .tokens
            .store_refresh_token(
                &state,
                &other,
                &record.mailbox_id,
                Sensitive::new("CANARY-refresh-token-0002".to_owned()),
            )
            .await,
        Err(ApiError::NotFound)
    );
    Ok(())
}

#[tokio::test]
async fn api_refresh_token_maps_missing_and_revoked() -> Result<(), Box<dyn std::error::Error>> {
    let (state, fakes) = fixture()?;
    let user = seed_user(&fakes).await?;
    let absent = MailboxId(fakes.rng.uuid_v4());
    assert_eq!(
        state
            .tokens
            .refresh_token(&state, &user, &absent)
            .await
            .err(),
        Some(ApiError::NotFound)
    );

    let record = seed_mailbox(&fakes, &user, MailboxStatus::Connected).await?;
    assert_eq!(
        state
            .tokens
            .refresh_token(&state, &user, &record.mailbox_id)
            .await
            .err(),
        Some(ApiError::MailboxNeedsSignIn {
            mailbox_id: Some(record.mailbox_id.0)
        })
    );

    let connected = seed_connected(&state, &fakes, &user, CANARY_REFRESH).await?;
    let token = state
        .tokens
        .refresh_token(&state, &user, &connected.mailbox_id)
        .await?;
    assert_eq!(token.expose().as_str(), CANARY_REFRESH);

    // forget() drops the cached token for the mailbox.
    state.tokens.forget(&connected.mailbox_id);
    Ok(())
}
