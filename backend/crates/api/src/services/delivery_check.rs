//! Feed-load delivery monitoring. The hook is pure; publication follows the state write.
use crate::{error::ApiError, state::AppState};
use domain::delivery::{
    evaluate, judge_mail, BusinessCalendar, CheckEnd, MailVerdict, CHECK_LIFETIME_DAYS,
};
use domain::user_state::UserState;
use domain::{MailboxId, NeedsAttentionReason, UserId};
use std::collections::BTreeSet;
use time::{Date, Duration, OffsetDateTime};

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
                ignored.push(IgnoredUnsubscribe {
                    mailbox_id: check.mailbox_id,
                    sender_display: mail.sender_display.clone(),
                });
                return false;
            }
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
        svc_common::needs_attention::raise_item(
            &app.ports,
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
