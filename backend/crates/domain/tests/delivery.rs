//! T-707 delivery boundaries.
use domain::delivery::{evaluate, judge_mail, BusinessCalendar, CheckEnd, MailVerdict};
use domain::user_state::PendingDeliveryCheck;
use domain::MailboxId;
use std::collections::BTreeSet;
use time::{
    macros::{date, datetime},
    Duration,
};
use uuid::Uuid;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn un_06_ac1_mail_at_exactly_5_business_days_no_item() -> TestResult {
    let cal = BusinessCalendar::new(BTreeSet::new());
    let sent = datetime!(2026-10-05 09:00 +10);
    assert_eq!(
        judge_mail(&cal, sent, datetime!(2026-10-12 09:00 +10)),
        MailVerdict::WithinGrace
    );
    assert_eq!(
        judge_mail(&cal, sent, datetime!(2026-10-12 09:01 +10)),
        MailVerdict::Ignored
    );
    assert_eq!(sent.offset(), time::UtcOffset::from_hms(10, 0, 0)?);
    Ok(())
}

#[test]
fn un_06_ac1_weekend_and_holiday_skipped() -> TestResult {
    let cal = BusinessCalendar::new(BTreeSet::from([
        date!(2026 - 12 - 25),
        date!(2026 - 12 - 28),
        date!(2027 - 01 - 01),
    ]));
    assert_eq!(
        cal.add_business_days(datetime!(2026-12-24 10:00 +10), 5),
        datetime!(2027-01-05 10:00 +10)
    );
    assert!(!cal.is_business_day(time::Date::from_calendar_date(
        2026,
        time::Month::December,
        25
    )?));
    Ok(())
}

#[test]
fn business_calendar_add_days_table() -> TestResult {
    let cal = BusinessCalendar::new(BTreeSet::from([
        date!(2026 - 12 - 25),
        date!(2026 - 12 - 28),
    ]));
    for (from, days, expected) in [
        (
            datetime!(2026-10-05 09:00 +10),
            5,
            datetime!(2026-10-12 09:00 +10),
        ),
        (
            datetime!(2026-10-09 09:00 +10),
            1,
            datetime!(2026-10-12 09:00 +10),
        ),
        (
            datetime!(2026-10-10 09:00 +10),
            1,
            datetime!(2026-10-13 09:00 +10),
        ),
        (
            datetime!(2026-12-24 10:00 +10),
            1,
            datetime!(2026-12-29 10:00 +10),
        ),
        (
            datetime!(2026-10-04 23:00 UTC),
            5,
            datetime!(2026-10-12 09:00 +10),
        ),
    ] {
        assert_eq!(cal.add_business_days(from, days), expected);
    }
    assert!(!cal.is_business_day(time::Date::from_calendar_date(
        2026,
        time::Month::October,
        10
    )?));
    Ok(())
}

#[test]
fn st_02_ac1_evaluate_boundaries() -> TestResult {
    let sent = datetime!(2026-10-05 09:00 +10);
    let mut check = PendingDeliveryCheck {
        sender_key: "list@example.com".into(),
        list_id: None,
        mailbox_id: MailboxId(Uuid::nil()),
        unsubscribed_at: sent,
        mail_seen: false,
        confirm_counted: false,
        pending_ignored_display: None,
    };
    assert_eq!(
        evaluate(&check, sent + Duration::days(14) - Duration::seconds(1)),
        CheckEnd::Keep
    );
    assert_eq!(
        evaluate(&check, sent + Duration::days(14)),
        CheckEnd::Confirmed
    );
    check.mail_seen = true;
    assert_eq!(
        evaluate(&check, sent + Duration::days(14)),
        CheckEnd::NotConfirmed
    );
    check.confirm_counted = true;
    assert_eq!(evaluate(&check, sent + Duration::days(15)), CheckEnd::Keep);
    assert_eq!(
        serde_json::from_slice::<PendingDeliveryCheck>(&serde_json::to_vec(&check)?)?,
        check
    );
    Ok(())
}

#[test]
fn delivery_old_state_defaults() -> TestResult {
    let check = PendingDeliveryCheck {
        sender_key: "list@example.com".into(),
        list_id: None,
        mailbox_id: MailboxId(Uuid::nil()),
        unsubscribed_at: datetime!(2026-10-05 09:00 +10),
        mail_seen: true,
        confirm_counted: true,
        pending_ignored_display: Some("Synthetic".into()),
    };
    let mut value = serde_json::to_value(check)?;
    let obj = value.as_object_mut().ok_or("object")?;
    obj.remove("mail_seen");
    obj.remove("confirm_counted");
    obj.remove("pending_ignored_display");
    let old: PendingDeliveryCheck = serde_json::from_value(value)?;
    assert!(!old.mail_seen);
    assert!(!old.confirm_counted);
    assert!(old.pending_ignored_display.is_none());
    Ok(())
}
