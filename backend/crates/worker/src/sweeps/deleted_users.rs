//! T-803 step 11: finish the deletion of users whose account deletion left
//! records behind (S2 AU-06 AC1).
//!
//! A record is an orphan when its `user_id` has no user document. The request
//! deletes the user record first, so a failure after that point leaves
//! orphans, never a half-deleted user who can still sign in. T-706 will call
//! [`sweep_deleted_users`] from API-INT-2; nothing calls it yet.

use std::collections::BTreeSet;

use domain::UserId;
use ports::{ServerStore, StoreError};
use time::OffsetDateTime;

/// Hours within which every leftover record is gone (S2 AU-06 AC1).
pub const DELETION_SWEEP_HOURS: i64 = 24;

/// A moment after every record's `expires_at`, so `expires_by` lists records
/// oldest-expiry first whatever their age.
const HORIZON_UNIX: i64 = 4_102_444_800;

/// Delete every job, Needs Attention item, mailbox and session whose user has
/// no user document. Returns how many records were deleted.
///
/// Orphans are found through the jobs, Needs Attention items and sessions
/// (each scan reads at most `limit` records, so a run is bounded), then every
/// collection is cleared for each orphaned user, mailboxes included.
///
/// # Errors
///
/// `StoreError` from the first failing store call; the next run retries.
pub async fn sweep_deleted_users(store: &dyn ServerStore, limit: u32) -> Result<u64, StoreError> {
    let horizon = OffsetDateTime::from_unix_timestamp(HORIZON_UNIX)
        .map_err(|_| StoreError::Invalid("sweep horizon"))?;
    let mut owners: BTreeSet<UserId> = BTreeSet::new();
    for job in store.jobs().expires_by(horizon, limit).await? {
        owners.insert(job.record.user_id);
    }
    for id in store.needs_attention().expires_by(horizon, limit).await? {
        if let Some(item) = store.needs_attention().get(&id).await? {
            owners.insert(item.record.user_id);
        }
    }
    for hash in store.sessions().expires_by(horizon, limit).await? {
        if let Some(session) = store.sessions().get(&hash).await? {
            if let Some(user) = session.record.user_id {
                owners.insert(user);
            }
        }
    }

    let mut deleted: u64 = 0;
    for user in owners {
        if store.users().get(&user).await?.is_some() {
            continue;
        }
        deleted = deleted
            .saturating_add(store.mailboxes().delete_all_for_user(&user).await?)
            .saturating_add(store.jobs().delete_all_for_user(&user).await?)
            .saturating_add(store.needs_attention().delete_all_for_user(&user).await?)
            .saturating_add(store.sessions().delete_all_for_user(&user).await?);
    }
    Ok(deleted)
}
