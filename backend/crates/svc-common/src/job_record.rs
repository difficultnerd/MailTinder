//! Sealing and opening the `target` and `sender_display` fields of a job
//! record (T-701 trap 3).
//!
//! A job's target (an https unsubscribe URL or a mailto URI) and its sender
//! display are stored only as AEAD ciphertext under the owning user's
//! KMS-wrapped `data_key`, bound to the job ID and the field name. The
//! decrypted values never leave the process and are never logged (S5 JOB-1,
//! S6 6).

use domain::{JobId, UserId};
use obs::Sensitive;
use ports::store::{aad_fields, Ciphertext, JobRecord};
use ports::{Aad, Ports};

use crate::error::SvcError;

/// Seal a job's `target`.
pub async fn seal_target(
    ports: &Ports,
    user: &UserId,
    job_id: &JobId,
    target: &Sensitive<String>,
) -> Result<Ciphertext, SvcError> {
    seal_field(ports, user, job_id, aad_fields::JOB_TARGET, target.expose()).await
}

/// Seal a job's `sender_display`.
pub async fn seal_sender_display(
    ports: &Ports,
    user: &UserId,
    job_id: &JobId,
    sender_display: &Sensitive<String>,
) -> Result<Ciphertext, SvcError> {
    seal_field(
        ports,
        user,
        job_id,
        aad_fields::JOB_SENDER_DISPLAY,
        sender_display.expose(),
    )
    .await
}

/// Open a job's `target`. Missing ciphertext is `NotFound`.
pub async fn open_target(
    ports: &Ports,
    user: &UserId,
    job: &JobRecord,
) -> Result<Sensitive<String>, SvcError> {
    let some = job.target.as_ref().ok_or(SvcError::NotFound)?;
    open_field(ports, user, &job.job_id, aad_fields::JOB_TARGET, some).await
}

/// Open a job's `sender_display`. Missing ciphertext is `NotFound`.
pub async fn open_sender_display(
    ports: &Ports,
    user: &UserId,
    job: &JobRecord,
) -> Result<Sensitive<String>, SvcError> {
    let some = job.sender_display.as_ref().ok_or(SvcError::NotFound)?;
    open_field(
        ports,
        user,
        &job.job_id,
        aad_fields::JOB_SENDER_DISPLAY,
        some,
    )
    .await
}

async fn seal_field(
    ports: &Ports,
    user: &UserId,
    job_id: &JobId,
    field: &'static str,
    plaintext: &str,
) -> Result<Ciphertext, SvcError> {
    let wrapped = wrapped_key(ports, user).await?;
    let aad = aad(user, job_id, field);
    ports
        .keys
        .seal(user, &wrapped, &aad, plaintext.as_bytes())
        .await
        .map(Ciphertext)
        .map_err(|_| SvcError::Crypto)
}

async fn open_field(
    ports: &Ports,
    user: &UserId,
    job_id: &JobId,
    field: &'static str,
    ciphertext: &Ciphertext,
) -> Result<Sensitive<String>, SvcError> {
    let wrapped = wrapped_key(ports, user).await?;
    let aad = aad(user, job_id, field);
    let plaintext = ports
        .keys
        .open(user, &wrapped, &aad, &ciphertext.0)
        .await
        .map_err(|_| SvcError::Crypto)?;
    String::from_utf8(plaintext)
        .map(Sensitive::new)
        .map_err(|_| SvcError::Crypto)
}

fn aad(user: &UserId, job_id: &JobId, field: &'static str) -> Aad {
    Aad {
        user: *user,
        scope: job_id.0.to_string(),
        field,
    }
}

async fn wrapped_key(ports: &Ports, user: &UserId) -> Result<ports::WrappedKey, SvcError> {
    Ok(ports
        .store
        .users()
        .get(user)
        .await
        .map_err(|_| SvcError::Store)?
        .ok_or(SvcError::NotFound)?
        .record
        .wrapped_data_key)
}
