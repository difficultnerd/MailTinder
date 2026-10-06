//! T-803 step 11: finish the deletion of users whose account deletion left
//! records behind (S2 AU-06 AC1).
//!
//! A record is an orphan when its `user_id` has no user document. The request
//! deletes the user record first, so a failure after that point leaves
//! orphans, never a half-deleted user who can still sign in. The worker's
//! sweep entry point ([`crate::sweeps::run_sweeps`]) calls
//! [`sweep_deleted_users`]; T-706's API-INT-2 route is what calls the entry
//! point in production (it does not exist yet).
//!
//! Orphans are found through the jobs, Needs Attention items, sessions and
//! mailboxes, each scanned in pages: `expires_at` ascending for the first
//! three, `linked_at` ascending for mailboxes. Paging matters — a single
//! `limit`-sized window ordered oldest-first can be filled by live users'
//! records, leaving an orphan beyond it unreachable however often the sweep
//! runs. The mailbox scan is what finds a user whose only leftover is a
//! mailbox whose delete failed. Evaluation records are keyed by the
//! pseudonymous ID, so they are deleted per discovered user.

use std::collections::BTreeSet;

use domain::UserId;
use obs::Pseudonymiser;
use ports::{PageRequest, ServerStore, StoreError, UserPseudoId};
use time::OffsetDateTime;

/// Hours within which every leftover record is gone (S2 AU-06 AC1).
pub const DELETION_SWEEP_HOURS: i64 = 24;

/// Pages read per collection per run. A run is bounded (S4 1): at most
/// `SWEEP_MAX_PAGES * limit` records of one collection are examined, and the
/// next run continues from the start with the orphans of the last run gone.
pub const SWEEP_MAX_PAGES: u32 = 20;

/// A moment after every record's `expires_at`, so `expires_by` lists records
/// oldest-expiry first whatever their age.
const HORIZON_UNIX: i64 = 4_102_444_800;

/// Delete every job, Needs Attention item, mailbox, session and evaluation
/// record whose user has no user document. Returns how many records were
/// deleted.
///
/// # Errors
///
/// `StoreError` from the first failing store call; the next run retries.
pub async fn sweep_deleted_users(
    store: &dyn ServerStore,
    limit: u32,
    pseudo: &Pseudonymiser,
) -> Result<u64, StoreError> {
    let horizon = OffsetDateTime::from_unix_timestamp(HORIZON_UNIX)
        .map_err(|_| StoreError::Invalid("sweep horizon"))?;
    let mut owners: BTreeSet<UserId> = BTreeSet::new();

    let mut after = None;
    for _ in 0..SWEEP_MAX_PAGES {
        let page = store
            .jobs()
            .expires_by(horizon, PageRequest { limit, after })
            .await?;
        for job in &page.items {
            owners.insert(job.record.user_id);
        }
        match page.next {
            Some(cursor) => after = Some(cursor),
            None => break,
        }
    }

    let mut after = None;
    for _ in 0..SWEEP_MAX_PAGES {
        let page = store
            .needs_attention()
            .expires_by(horizon, PageRequest { limit, after })
            .await?;
        for id in &page.items {
            if let Some(item) = store.needs_attention().get(id).await? {
                owners.insert(item.record.user_id);
            }
        }
        match page.next {
            Some(cursor) => after = Some(cursor),
            None => break,
        }
    }

    let mut after = None;
    for _ in 0..SWEEP_MAX_PAGES {
        let page = store
            .sessions()
            .expires_by(horizon, PageRequest { limit, after })
            .await?;
        for hash in &page.items {
            if let Some(session) = store.sessions().get(hash).await? {
                if let Some(user) = session.record.user_id {
                    owners.insert(user);
                }
            }
        }
        match page.next {
            Some(cursor) => after = Some(cursor),
            None => break,
        }
    }

    // A mailbox whose user is gone is an orphan in its own right: a deletion
    // whose mailbox delete failed leaves no other record behind when the user
    // had no jobs, items or sessions (T-803 F3).
    let mut after = None;
    for _ in 0..SWEEP_MAX_PAGES {
        let page = store.mailboxes().list(PageRequest { limit, after }).await?;
        for mailbox in &page.items {
            owners.insert(mailbox.record.user_id);
        }
        match page.next {
            Some(cursor) => after = Some(cursor),
            None => break,
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
            .saturating_add(store.sessions().delete_all_for_user(&user).await?)
            .saturating_add(delete_evals(store, pseudo, &user).await?);
    }
    Ok(deleted)
}

/// Delete the user's evaluation records. They hold the pseudonymous ID, not
/// the user ID, so they cannot be found by the scans above.
async fn delete_evals(
    store: &dyn ServerStore,
    pseudo: &Pseudonymiser,
    user: &UserId,
) -> Result<u64, StoreError> {
    let id = pseudo.pseudo_id(&user.0);
    store
        .classifier_eval()
        .delete_for_users(&[UserPseudoId(id.as_str().to_owned())])
        .await
}
