//! Pure delivery-check decisions with an injected Australian business calendar.
use crate::user_state::PendingDeliveryCheck;
use std::collections::BTreeSet;
use time::{Date, Duration, OffsetDateTime, UtcOffset, Weekday};

pub const GRACE_BUSINESS_DAYS: u32 = 5;
pub const CONFIRM_DAYS: i64 = 14;
pub const CHECK_LIFETIME_DAYS: i64 = 30;
pub const AU_OFFSET_HOURS: i8 = 10;

pub struct BusinessCalendar {
    holidays: BTreeSet<Date>,
}
impl BusinessCalendar {
    pub fn new(holidays: BTreeSet<Date>) -> Self {
        Self { holidays }
    }
    pub fn is_business_day(&self, d: Date) -> bool {
        !matches!(d.weekday(), Weekday::Saturday | Weekday::Sunday) && !self.holidays.contains(&d)
    }
    /// Keep the UTC+10 local time; a weekend start first moves to a business day.
    pub fn add_business_days(&self, from: OffsetDateTime, n: u32) -> OffsetDateTime {
        // This constant offset is representable; never consult a system timezone.
        let offset = UtcOffset::from_hms(AU_OFFSET_HOURS, 0, 0).unwrap_or(UtcOffset::UTC);
        let mut local = from.to_offset(offset);
        while !self.is_business_day(local.date()) {
            let Some(next) = local.checked_add(Duration::days(1)) else {
                return local;
            };
            local = next;
        }
        let mut remaining = n;
        while remaining > 0 {
            let Some(next) = local.checked_add(Duration::days(1)) else {
                return local;
            };
            local = next;
            if self.is_business_day(local.date()) {
                remaining -= 1;
            }
        }
        local
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MailVerdict {
    WithinGrace,
    Ignored,
}
pub fn judge_mail(
    cal: &BusinessCalendar,
    unsubscribed_at: OffsetDateTime,
    received_at: OffsetDateTime,
) -> MailVerdict {
    if received_at > cal.add_business_days(unsubscribed_at, GRACE_BUSINESS_DAYS) {
        MailVerdict::Ignored
    } else {
        MailVerdict::WithinGrace
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CheckEnd {
    Confirmed,
    NotConfirmed,
    Keep,
}
pub fn evaluate(check: &PendingDeliveryCheck, now: OffsetDateTime) -> CheckEnd {
    if check.confirm_counted || now - check.unsubscribed_at < Duration::days(CONFIRM_DAYS) {
        CheckEnd::Keep
    } else if check.mail_seen {
        CheckEnd::NotConfirmed
    } else {
        CheckEnd::Confirmed
    }
}
