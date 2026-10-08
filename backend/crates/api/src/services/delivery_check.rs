//! Feed-load delivery monitoring. The hook is pure; publication follows the state write.
use crate::services::list_key::HmacKey;
use crate::services::user_state_store::UserStateStore;
use crate::{error::ApiError, state::AppState};
use domain::delivery::{
    evaluate, judge_mail, BusinessCalendar, CheckEnd, MailVerdict, CHECK_LIFETIME_DAYS,
};
use domain::user_state::UserState;
use domain::{MailboxId, NeedsAttentionReason, UserId};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::collections::BTreeSet;
use std::sync::Arc;
use time::{Date, Duration, OffsetDateTime};
use uuid::Uuid;

pub struct DeliveryHookInput<'a> {
    pub list_mail: &'a [ListMail],
}
#[derive(Clone)]
pub struct ListMail {
    pub sender_key: String,
    pub list_id: Option<String>,
    pub mailbox_id: MailboxId,
    pub received_at: OffsetDateTime,
    pub sender_display: String,
}
pub struct IgnoredUnsubscribe {
    pub mailbox_id: MailboxId,
    pub sender_display: String,
    pub sender_key: String,
    pub list_id: Option<String>,
    pub unsubscribed_at: OffsetDateTime,
}

#[must_use]
pub fn apply_delivery_checks(
    state: &mut UserState,
    cal: &BusinessCalendar,
    now: OffsetDateTime,
    input: &DeliveryHookInput<'_>,
) -> Vec<IgnoredUnsubscribe> {
    let mut ignored = Vec::new();
    state.pending_delivery_checks.retain_mut(|check| {
        for mail in input.list_mail {
            if mail.sender_key != check.sender_key
                || mail.list_id != check.list_id
                || mail.received_at <= check.unsubscribed_at
            {
                continue;
            }
            check.mail_seen = true;
            if judge_mail(cal, check.unsubscribed_at, mail.received_at) == MailVerdict::Ignored {
                if check.pending_ignored_display.is_none() {
                    check.pending_ignored_display = Some(mail.sender_display.clone());
                }
                break;
            }
        }
        if let Some(display) = &check.pending_ignored_display {
            ignored.push(IgnoredUnsubscribe {
                mailbox_id: check.mailbox_id,
                sender_display: display.clone(),
                sender_key: check.sender_key.clone(),
                list_id: check.list_id.clone(),
                unsubscribed_at: check.unsubscribed_at,
            });
            // Keep failed publications even after the monitoring lifetime expires.
            return true;
        }
        if evaluate(check, now) == CheckEnd::Confirmed {
            state.totals.unsubscribes_confirmed =
                state.totals.unsubscribes_confirmed.saturating_add(1);
            check.confirm_counted = true;
        }
        now - check.unsubscribed_at <= Duration::days(CHECK_LIFETIME_DAYS)
    });
    ignored
}

/// Parse the bundled, restricted TOML date list once at application construction.
#[must_use]
pub fn national_calendar() -> BusinessCalendar {
    let holidays: BTreeSet<Date> = include_str!("../../config/au_holidays.toml")
        .lines()
        .filter(|line| !line.trim_start().starts_with('#'))
        .flat_map(|line| {
            line.split('"')
                .enumerate()
                .filter_map(|(i, token)| (i % 2 == 1).then_some(token))
        })
        .filter_map(|token| {
            Date::parse(
                token,
                &time::macros::format_description!("[year]-[month]-[day]"),
            )
            .ok()
        })
        .collect();
    BusinessCalendar::new(holidays)
}

/// Publish only outcomes returned by the successful update attempt.
///
/// # Errors
/// Returns an internal error if the encrypted Needs Attention write fails.
pub async fn publish(
    app: &AppState,
    user: &UserId,
    ignored: &[IgnoredUnsubscribe],
    confirmed: u64,
) -> Result<(), ApiError> {
    for item in ignored {
        // Stable across Feed loads and concurrent publishers, without persisting
        // any list identifier in the server record. The id is a keyed HMAC (the
        // email lookup key, as the list-key hash uses) so that read access to
        // `needs_attention/{id}` cannot be used to test guesses of the sender or
        // List-Id; an unkeyed hash would allow exactly that (S5:33, T-707 edge
        // cases: nothing about the list reaches the server store).
        let item_id = item_id(app, user, item)?;
        svc_common::needs_attention::raise_item_with_id(
            &app.ports,
            item_id,
            svc_common::needs_attention::NewItem {
                user,
                mailbox: &item.mailbox_id,
                sender_display: &item.sender_display,
                link: None,
                reason: NeedsAttentionReason::UnsubscribeIgnored,
            },
        )
        .await
        .map_err(|_| ApiError::Internal)?;
        UserStateStore::new(Arc::new(app.clone()))
            .update(user, |state| {
                state.pending_delivery_checks.retain(|check| {
                    !(check.mailbox_id == item.mailbox_id
                        && check.sender_key == item.sender_key
                        && check.list_id == item.list_id
                        && check.unsubscribed_at == item.unsubscribed_at
                        && check.pending_ignored_display.is_some())
                });
            })
            .await?;
        metric("ignored");
    }
    for _ in 0..confirmed {
        metric("confirmed");
    }
    Ok(())
}

/// The server-side key of `needs_attention/{id}` (S5). Derived with a keyed
/// HMAC over the item's identity so the id is stable but not guessable from a
/// readable store; the `needs-attention:` domain keeps it separate from the
/// list-key HMAC that shares the email lookup key.
fn item_id(
    app: &AppState,
    user: &UserId,
    item: &IgnoredUnsubscribe,
) -> Result<ports::store::NeedsAttentionId, ApiError> {
    let key = HmacKey(app.config.email_lookup_key.clone());
    let mut mac = Hmac::<Sha256>::new_from_slice(key.0.expose()).map_err(|_| ApiError::Internal)?;
    mac.update(b"needs-attention:");
    mac.update(user.0.as_bytes());
    mac.update(b"\n");
    mac.update(item.mailbox_id.0.as_bytes());
    mac.update(b"\n");
    mac.update(item.sender_key.as_bytes());
    mac.update(b"\n");
    mac.update(item.list_id.as_deref().unwrap_or_default().as_bytes());
    mac.update(b"\n");
    mac.update(&item.unsubscribed_at.unix_timestamp_nanos().to_be_bytes());
    let digest = mac.finalize().into_bytes();
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    Ok(ports::store::NeedsAttentionId(Uuid::from_bytes(bytes)))
}

fn metric(outcome: &'static str) {
    obs::metric_event(&obs::MetricEvent {
        event_type: "delivery_check_outcome",
        outcome,
        user: None,
        provider: None,
    });
}
