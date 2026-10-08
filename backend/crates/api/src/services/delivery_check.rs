//! Feed-load delivery monitoring. The hook is pure; publication follows the state write.
use crate::services::user_state_store::UserStateStore;
use crate::{error::ApiError, state::AppState};
use domain::delivery::{
    evaluate, judge_mail, BusinessCalendar, CheckEnd, MailVerdict, CHECK_LIFETIME_DAYS,
};
use domain::user_state::UserState;
use domain::{MailboxId, NeedsAttentionReason, UserId};
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
        // any list identifier in the server record.
        let identity = serde_json::to_vec(&(
            user,
            item.mailbox_id,
            &item.sender_key,
            &item.list_id,
            item.unsubscribed_at,
        ))
        .map_err(|_| ApiError::Internal)?;
        let item_id = ports::store::NeedsAttentionId(Uuid::new_v5(
            &Uuid::from_u128(0x76f66c72_574b_57f9_93f3_499ec0fe0707),
            &identity,
        ));
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
fn metric(outcome: &'static str) {
    obs::metric_event(&obs::MetricEvent {
        event_type: "delivery_check_outcome",
        outcome,
        user: None,
        provider: None,
    });
}
