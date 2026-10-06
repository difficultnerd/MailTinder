//! T-507 `mt-admin` command tests (ASVS V6.3.2, AU-01 AC1) with testkit fakes.
//!
//! No network or GCP: the store, keys, clock and rng are the deterministic
//! fakes from `testkit`.
#![allow(clippy::too_many_lines)]

use admin_cli::commands::{
    invite, list_users, make_admin, CliError, Deps, MakeAdminResult, UserLine,
};
use domain::{EmailAddress, InviteStatus, MailboxStatus, Provider, ProviderSubjectId, UserId};
use obs::Sensitive;
use ports::store::aad_fields;
use ports::{
    Ciphertext, Clock, KeyService, MailboxRecord, Precondition, Rng, ServerStore, SystemAad,
    SystemKeyService, UserRecord,
};
use sha2::{Digest, Sha256};
use svc_common::invites::email_lookup_hash;
use testkit::{fake_ports, Fakes};
use time::Duration;
use url::Url;

type TestResult = Result<(), Box<dyn std::error::Error>>;

const ORIGIN: &str = "https://mailtinder.test";
const EMAIL_KEY: &[u8] = b"fake-email-key";
const INVITEE: &str = "invitee@example.com";

/// Ports plus the raw keys and origin the `Deps` borrow.
struct Fixture {
    ports: ports::Ports,
    fakes: Fakes,
    email_key: Sensitive<Vec<u8>>,
    pseudo_key: Sensitive<Vec<u8>>,
    origin: Url,
}

impl Fixture {
    fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let (ports, fakes) = fake_ports();
        Ok(Self {
            ports,
            fakes,
            email_key: Sensitive::new(EMAIL_KEY.to_vec()),
            pseudo_key: Sensitive::new(b"fake-log-key".to_vec()),
            origin: Url::parse(ORIGIN)?,
        })
    }

    fn deps(&self) -> Deps<'_> {
        Deps {
            store: self.ports.store.as_ref(),
            system_keys: self.ports.system_keys.as_ref(),
            email_lookup_key: &self.email_key,
            clock: self.ports.clock.as_ref(),
            rng: self.ports.rng.as_ref(),
            app_origin: &self.origin,
            pseudo_key: &self.pseudo_key,
        }
    }
}

fn sha(token: &str) -> ports::Sha256Hash {
    let digest = Sha256::digest(token.as_bytes());
    let mut out = [0u8; 32];
    out.copy_from_slice(&digest);
    ports::Sha256Hash(out)
}

/// The raw `t` token from the invite link fragment.
fn token_of(link: &Url) -> Result<String, Box<dyn std::error::Error>> {
    let fragment = link.fragment().ok_or("link fragment")?;
    let token = fragment.split("t=").nth(1).ok_or("t parameter")?;
    Ok(token.to_owned())
}

async fn seed_user(fakes: &Fakes, is_admin: bool) -> Result<UserId, Box<dyn std::error::Error>> {
    let user = UserId::new(fakes.rng.uuid_v4());
    let wrapped = fakes.keys.new_user_key(&user).await?;
    fakes
        .store
        .users()
        .put(
            &UserRecord {
                user_id: user,
                created_at: fakes.clock.now(),
                is_admin,
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
    user: UserId,
    address: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let subject = ProviderSubjectId::new("seed-subject-1")?;
    let mailbox_id = ports::mailbox_id_for(Provider::Gmail, &subject);
    fakes
        .store
        .mailboxes()
        .put(
            &MailboxRecord {
                mailbox_id,
                user_id: user,
                provider: Provider::Gmail,
                provider_subject_id: subject,
                email_address: Ciphertext(address.as_bytes().to_vec()),
                status: MailboxStatus::Connected,
                linked_at: fakes.clock.now(),
                is_primary: true,
                refresh_token: None,
            },
            Precondition::MustNotExist,
        )
        .await?;
    Ok(())
}

#[tokio::test]
async fn asvs_v6_3_2_no_admin_until_make_admin() -> TestResult {
    let f = Fixture::new()?;
    // A fresh store has no users, so no admin.
    assert!(list_users(&f.deps()).await?.iter().all(|u| !u.is_admin));

    let user = seed_user(&f.fakes, false).await?;
    assert!(list_users(&f.deps()).await?.iter().all(|u| !u.is_admin));

    assert_eq!(
        make_admin(&f.deps(), &user).await?,
        MakeAdminResult::Granted
    );
    let after = list_users(&f.deps()).await?;
    assert_eq!(after.iter().filter(|u| u.is_admin).count(), 1);
    assert_eq!(after[0].user_id, user);
    Ok(())
}

#[tokio::test]
async fn asvs_v6_3_2_make_admin_unknown_user_not_found() -> TestResult {
    let f = Fixture::new()?;
    let missing = UserId::new(f.fakes.rng.uuid_v4());
    assert!(matches!(
        make_admin(&f.deps(), &missing).await,
        Err(CliError::NotFound)
    ));
    Ok(())
}

#[tokio::test]
async fn asvs_v6_3_2_make_admin_twice_is_already_admin() -> TestResult {
    let f = Fixture::new()?;
    let user = seed_user(&f.fakes, false).await?;
    assert_eq!(
        make_admin(&f.deps(), &user).await?,
        MakeAdminResult::Granted
    );
    assert_eq!(
        make_admin(&f.deps(), &user).await?,
        MakeAdminResult::AlreadyAdmin
    );
    Ok(())
}

#[tokio::test]
async fn au_01_ac1_cli_invite_redeemable() -> TestResult {
    let f = Fixture::new()?;
    let email = EmailAddress::parse(INVITEE)?;
    let link = invite(&f.deps(), &email).await?;
    let token = token_of(&link)?;

    let stored = f
        .fakes
        .store
        .invites()
        .by_token_hash(&sha(&token))
        .await?
        .ok_or("invite stored by token hash")?;
    assert_eq!(stored.record.status, InviteStatus::Pending);
    assert_eq!(
        stored.record.email_lookup,
        email_lookup_hash(&f.email_key, &email)
    );

    // The address is sealed with the system key under the invite's own ID.
    let opened = f
        .fakes
        .system_keys
        .open(
            &SystemAad {
                scope: stored.record.invite_id.0.to_string(),
                field: aad_fields::INVITE_EMAIL,
            },
            &stored.record.email_address.0,
        )
        .await?;
    assert_eq!(opened, INVITEE.as_bytes());

    // A normal 7-day pending invite with its purge time.
    assert_eq!(
        stored.record.expires_at,
        f.fakes.clock.now() + Duration::days(7)
    );
    assert!(stored.record.purge_at > stored.record.expires_at);
    Ok(())
}

#[tokio::test]
async fn au_01_ac1_cli_invite_twice_resends() -> TestResult {
    let f = Fixture::new()?;
    let email = EmailAddress::parse(INVITEE)?;
    let first = token_of(&invite(&f.deps(), &email).await?)?;
    let second = token_of(&invite(&f.deps(), &email).await?)?;
    assert_ne!(first, second);

    let pending = f
        .fakes
        .store
        .invites()
        .by_email_lookup(&email_lookup_hash(&f.email_key, &email))
        .await?;
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].record.token_hash, sha(&second));
    Ok(())
}

#[tokio::test]
async fn list_users_prints_no_email() -> TestResult {
    let f = Fixture::new()?;
    let user = seed_user(&f.fakes, false).await?;
    seed_mailbox(&f.fakes, user, "admin@example.com").await?;

    let lines = list_users(&f.deps()).await?;
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0].mailbox_count, 1);

    let rendered: String = lines
        .iter()
        .map(UserLine::line)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(!rendered.contains('@'));
    Ok(())
}
