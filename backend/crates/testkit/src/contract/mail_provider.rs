//! The shared `MailProvider` contract suite.

use std::sync::Arc;

use async_trait::async_trait;
use domain::{LabelSet, MessageId};
use ports::{ListOrder, MailProvider, MailboxCtx, MessagePage, MessageQuery, PageToken};

use crate::mailbox::{state::SeedMessage, SentRecord};

/// A seeder that can also read back labels and the sent log.
#[async_trait]
pub trait MailSeeder: Send + Sync {
    async fn seed(&self, msg: &SeedMessage) -> Result<MessageId, String>;
    /// Read back labels outside the adapter.
    async fn labels_of(&self, id: &MessageId) -> Result<LabelSet, String>;
    async fn sent(&self) -> Result<Vec<SentRecord>, String>;
    /// `FakeMailbox` always returns 0.
    async fn permanent_delete_attempts(&self) -> Result<u64, String>;
}

/// A target under test.
pub struct MailTarget {
    pub provider: Arc<dyn MailProvider>,
    pub ctx: MailboxCtx,
    pub seeder: Arc<dyn MailSeeder>,
}

/// Which case groups to run.
#[derive(Clone, Copy, Debug)]
pub struct CaseGroups {
    pub read: bool,
    pub modify: bool,
    pub send: bool,
}

impl CaseGroups {
    pub const ALL: CaseGroups = CaseGroups {
        read: true,
        modify: true,
        send: true,
    };
}

fn seed(from: &str, subject: &str, date_secs: i64, labels: &[&str]) -> SeedMessage {
    SeedMessage {
        from_display: "Sender".to_owned(),
        from_address: format!("{from}@example.com"),
        subject: subject.to_owned(),
        raw_headers: vec![],
        facts: domain::HeaderFacts {
            list_unsubscribe_present: true,
            ..domain::HeaderFacts::default()
        },
        preview_text: format!("CANARY-{subject}-preview"),
        internal_date: time::OffsetDateTime::from_unix_timestamp(date_secs)
            .unwrap_or_else(|_| panic!("date")),
        labels: labels.iter().map(|s| (*s).to_owned()).collect(),
    }
}

/// Verify the text-fetch extension preserves the preview contract.
///
/// # Errors
/// Returns the contract case name if either fetch fails or the text differs.
pub async fn get_text_matches_preview_at_300(t: &MailTarget) -> Result<(), String> {
    let mut message = seed("text", "text-contract", 100, &["INBOX"]);
    message.preview_text = "word e\u{301} 界 ".repeat(600);
    let id = t.seeder.seed(&message).await?;
    let preview = t
        .provider
        .get_preview(&t.ctx, &id)
        .await
        .map_err(|_| "get_text_matches_preview_at_300 preview")?;
    let text = t
        .provider
        .get_text(&t.ctx, &id, 300)
        .await
        .map_err(|_| "get_text_matches_preview_at_300 text")?;
    if preview != text {
        return Err("get_text_matches_preview_at_300 mismatch".to_owned());
    }
    let longer = t
        .provider
        .get_text(&t.ctx, &id, 4000)
        .await
        .map_err(|_| "get_text_matches_preview_at_300 longer")?;
    if longer.chars().count() <= text.chars().count()
        || domain::text::sanitise_plain(&longer, 300) != preview
    {
        return Err("get_text_matches_preview_at_300 extended text missing".to_owned());
    }
    if !t
        .provider
        .get_text(&t.ctx, &id, 0)
        .await
        .map_err(|_| "get_text_matches_preview_at_300 zero")?
        .is_empty()
    {
        return Err("get_text_matches_preview_at_300 zero cap".to_owned());
    }
    Ok(())
}

/// Each call to `make` returns a fresh, empty mailbox.
///
/// # Errors
///
/// Returns `Err` with the name of the first failing case.
pub async fn mail_provider<F, Fut>(make: F, groups: CaseGroups) -> Result<(), String>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = MailTarget>,
{
    if groups.read {
        let t = make().await;
        get_text_matches_preview_at_300(&t).await?;
        // Three messages with distinct dates come back newest first.
        {
            let t = make().await;
            let _ = t.seeder.seed(&seed("a", "old", 100, &["INBOX"])).await?;
            let _ = t.seeder.seed(&seed("b", "new", 300, &["INBOX"])).await?;
            let _ = t.seeder.seed(&seed("c", "mid", 200, &["INBOX"])).await?;
            let page = t
                .provider
                .list_inbox(&t.ctx, None, ListOrder::NewestFirst)
                .await
                .map_err(|e| format!("list: {e:?}"))?;
            let subjects: Vec<_> = page.items.iter().map(|m| m.subject.as_str()).collect();
            if subjects != vec!["new", "mid", "old"] {
                return Err(format!("newest-first order wrong: {subjects:?}"));
            }
            let count = t
                .provider
                .inbox_count(&t.ctx)
                .await
                .map_err(|e| format!("count: {e:?}"))?;
            if count != 3 {
                return Err("inbox_count wrong".into());
            }
        }

        // Paging with a page token covers all with no duplicates.
        {
            let t = make().await;
            for i in 0..25 {
                let _ = t
                    .seeder
                    .seed(&seed("p", &format!("m{i}"), 100 + i, &["INBOX"]))
                    .await?;
            }
            let mut seen = std::collections::HashSet::new();
            let mut token: Option<PageToken> = None;
            loop {
                let page: MessagePage = t
                    .provider
                    .list_inbox(&t.ctx, token, ListOrder::NewestFirst)
                    .await
                    .map_err(|e| format!("list: {e:?}"))?;
                for m in &page.items {
                    if !seen.insert(m.id.as_str().to_owned()) {
                        return Err("duplicate across pages".into());
                    }
                }
                match page.next {
                    Some(n) => token = Some(n),
                    None => break,
                }
            }
            if seen.len() != 25 {
                return Err(format!("paging covered {} not 25", seen.len()));
            }
        }

        // Unknown ID is NotFound.
        {
            let t = make().await;
            let _ = t.seeder.seed(&seed("a", "x", 100, &["INBOX"])).await?;
            let missing = MessageId::new("m9999").unwrap_or_else(|_| panic!("id"));
            match t.provider.get_meta(&t.ctx, &missing).await {
                Err(ports::MailError::NotFound) => {}
                _ => return Err("unknown meta should be NotFound".into()),
            }
        }

        // A trashed message is not listed. Trashing needs `trash`, so a
        // read-only target (T-401 ships the read half before T-403) skips it.
        if groups.modify {
            let t = make().await;
            let id = t
                .seeder
                .seed(&seed("a", "trashme", 100, &["INBOX"]))
                .await?;
            let _ = t
                .provider
                .trash(&t.ctx, &id)
                .await
                .map_err(|e| format!("trash: {e:?}"))?;
            let page = t
                .provider
                .list_inbox(&t.ctx, None, ListOrder::NewestFirst)
                .await
                .map_err(|e| format!("list: {e:?}"))?;
            if !page.items.is_empty() {
                return Err("trashed message still listed".into());
            }
        }

        // T-602a: typed queries over the inbox and date bounds.
        list_messages_inbox_and_dates(&make().await).await?;
    }

    if groups.modify {
        // T-602a: typed queries, counts and label changes.
        list_messages_label_and_sender(&make().await).await?;
        count_messages_label_exact(&make().await).await?;
        rename_label_cases(&make().await).await?;
        remove_label_keeps_messages(&make().await).await?;

        // trash then restore_labels(before) gives back exactly that set.
        {
            let t = make().await;
            let id = t
                .seeder
                .seed(&seed("a", "r", 100, &["INBOX", "UNREAD"]))
                .await?;
            let before = t
                .provider
                .trash(&t.ctx, &id)
                .await
                .map_err(|e| format!("trash: {e:?}"))?;
            t.provider
                .restore_labels(&t.ctx, &id, &before)
                .await
                .map_err(|e| format!("restore: {e:?}"))?;
            let read = t.seeder.labels_of(&id).await?;
            if read != before {
                return Err("restore did not give back the exact set".into());
            }
        }

        // set_labels with TRASH is Invalid.
        {
            let t = make().await;
            let id = t.seeder.seed(&seed("a", "s", 100, &["INBOX"])).await?;
            let trash = LabelSet::from_ids(vec!["TRASH".to_owned()]);
            match t
                .provider
                .set_labels(&t.ctx, &id, &trash, &LabelSet::new())
                .await
            {
                Err(ports::MailError::Invalid(_)) => {}
                _ => return Err("set_labels TRASH should be Invalid".into()),
            }
        }
    }

    if groups.send {
        // send_mailto records one sent message.
        {
            let t = make().await;
            let target =
                domain::MailtoTarget::new("victim@example.org", Some("bye"), Some("unsub"))
                    .unwrap_or_else(|_| panic!("mailto"));
            t.provider
                .send_mailto(&t.ctx, &target)
                .await
                .map_err(|e| format!("send: {e:?}"))?;
            let sent = t.seeder.sent().await?;
            if sent.len() != 1 || sent[0].to != "victim@example.org" {
                return Err("sent record wrong".into());
            }
        }
    }

    // Always: no permanent delete attempts.
    {
        let t = make().await;
        let attempts = t.seeder.permanent_delete_attempts().await?;
        if attempts != 0 {
            return Err(format!("permanent delete attempts: {attempts}"));
        }
    }

    Ok(())
}

/// The subjects of a page, in order.
fn subjects(page: &MessagePage) -> Vec<&str> {
    page.items.iter().map(|m| m.subject.as_str()).collect()
}

fn date(secs: i64) -> time::OffsetDateTime {
    time::OffsetDateTime::from_unix_timestamp(secs).unwrap_or_else(|_| panic!("date"))
}

async fn list(t: &MailTarget, q: &MessageQuery) -> Result<MessagePage, String> {
    t.provider
        .list_messages(&t.ctx, q, None, 100)
        .await
        .map_err(|e| format!("list_messages: {e:?}"))
}

fn expect_subjects(what: &str, page: &MessagePage, want: &[&str]) -> Result<(), String> {
    let got = subjects(page);
    if got == want {
        Ok(())
    } else {
        Err(format!("{what}: got {got:?}, want {want:?}"))
    }
}

/// XC-02: `list_messages` filters by inbox and date bounds, newest first, and
/// pages with a token. `t` is a fresh, empty mailbox.
///
/// # Errors
///
/// Returns `Err` describing the first wrong result.
pub async fn list_messages_inbox_and_dates(t: &MailTarget) -> Result<(), String> {
    let _ = t.seeder.seed(&seed("a", "old", 100, &["INBOX"])).await?;
    let _ = t.seeder.seed(&seed("b", "new", 300, &["INBOX"])).await?;
    let _ = t.seeder.seed(&seed("c", "mid", 200, &["INBOX"])).await?;
    let _ = t
        .seeder
        .seed(&seed("d", "archived", 250, &["UNREAD"]))
        .await?;

    let inbox = MessageQuery {
        in_inbox: true,
        ..MessageQuery::default()
    };
    expect_subjects("inbox", &list(t, &inbox).await?, &["new", "mid", "old"])?;

    let after = MessageQuery {
        after: Some(date(100)),
        ..inbox.clone()
    };
    expect_subjects("after is strict", &list(t, &after).await?, &["new", "mid"])?;

    let before = MessageQuery {
        before: Some(date(300)),
        ..inbox.clone()
    };
    expect_subjects(
        "before is strict",
        &list(t, &before).await?,
        &["mid", "old"],
    )?;

    let both = MessageQuery {
        after: Some(date(100)),
        before: Some(date(300)),
        ..inbox
    };
    expect_subjects("after and before", &list(t, &both).await?, &["mid"])?;

    // Paging: one at a time, newest first, no duplicates.
    let all = MessageQuery {
        in_inbox: true,
        ..MessageQuery::default()
    };
    let mut seen = Vec::new();
    let mut token: Option<PageToken> = None;
    for _ in 0..5 {
        let page = t
            .provider
            .list_messages(&t.ctx, &all, token, 1)
            .await
            .map_err(|e| format!("list_messages page: {e:?}"))?;
        seen.extend(page.items.iter().map(|m| m.subject.clone()));
        match page.next {
            Some(n) => token = Some(n),
            None => break,
        }
    }
    if seen != ["new", "mid", "old"] {
        return Err(format!("paging walked {seen:?}"));
    }
    Ok(())
}

/// XC-02: `list_messages` filters by label and sender. `t` is a fresh mailbox.
///
/// # Errors
///
/// Returns `Err` describing the first wrong result.
pub async fn list_messages_label_and_sender(t: &MailTarget) -> Result<(), String> {
    let a1 = t.seeder.seed(&seed("alice", "a1", 100, &["INBOX"])).await?;
    let b1 = t.seeder.seed(&seed("bob", "b1", 200, &["INBOX"])).await?;
    let _ = t.seeder.seed(&seed("alice", "a2", 300, &["INBOX"])).await?;
    let label = t
        .provider
        .ensure_label(&t.ctx, "Filed")
        .await
        .map_err(|e| format!("ensure_label: {e:?}"))?;
    let add = LabelSet::from_ids(vec![label.clone()]);
    for id in [&a1, &b1] {
        t.provider
            .set_labels(&t.ctx, id, &add, &LabelSet::new())
            .await
            .map_err(|e| format!("set_labels: {e:?}"))?;
    }

    let by_label = MessageQuery {
        label: Some(label.clone()),
        ..MessageQuery::default()
    };
    expect_subjects("label", &list(t, &by_label).await?, &["b1", "a1"])?;

    let by_sender = MessageQuery {
        from: Some("alice@example.com".to_owned()),
        ..MessageQuery::default()
    };
    expect_subjects("sender", &list(t, &by_sender).await?, &["a2", "a1"])?;

    let both = MessageQuery {
        label: Some(label),
        from: Some("alice@example.com".to_owned()),
        ..MessageQuery::default()
    };
    expect_subjects("label and sender", &list(t, &both).await?, &["a1"])?;
    Ok(())
}

/// XC-02: a label-only count is exact. `t` is a fresh mailbox.
///
/// # Errors
///
/// Returns `Err` describing the first wrong count.
pub async fn count_messages_label_exact(t: &MailTarget) -> Result<(), String> {
    let a = t.seeder.seed(&seed("a", "a", 100, &["INBOX"])).await?;
    let b = t.seeder.seed(&seed("b", "b", 200, &["INBOX"])).await?;
    let _ = t.seeder.seed(&seed("c", "c", 300, &["INBOX"])).await?;
    let label = t
        .provider
        .ensure_label(&t.ctx, "Counted")
        .await
        .map_err(|e| format!("ensure_label: {e:?}"))?;
    let add = LabelSet::from_ids(vec![label.clone()]);
    for id in [&a, &b] {
        t.provider
            .set_labels(&t.ctx, id, &add, &LabelSet::new())
            .await
            .map_err(|e| format!("set_labels: {e:?}"))?;
    }
    let count = |q: MessageQuery| async move {
        t.provider
            .count_messages(&t.ctx, &q)
            .await
            .map_err(|e| format!("count_messages: {e:?}"))
    };
    let labelled = count(MessageQuery {
        label: Some(label),
        ..MessageQuery::default()
    })
    .await?;
    if labelled != 2 {
        return Err(format!("label count {labelled}, want 2"));
    }
    let inbox = count(MessageQuery {
        in_inbox: true,
        ..MessageQuery::default()
    })
    .await?;
    if inbox != 3 {
        return Err(format!("inbox count {inbox}, want 3"));
    }
    let ranged = count(MessageQuery {
        in_inbox: true,
        after: Some(date(100)),
        before: Some(date(300)),
        ..MessageQuery::default()
    })
    .await?;
    if ranged != 1 {
        return Err(format!("date-range count {ranged}, want 1"));
    }
    Ok(())
}

/// XC-02: `rename_label` renames, refuses a clash with `label_exists` and
/// reports a missing label as `NotFound`. `t` is a fresh mailbox.
///
/// # Errors
///
/// Returns `Err` describing the first wrong result.
pub async fn rename_label_cases(t: &MailTarget) -> Result<(), String> {
    let old = t
        .provider
        .ensure_label(&t.ctx, "Before")
        .await
        .map_err(|e| format!("ensure_label: {e:?}"))?;
    t.provider
        .rename_label(&t.ctx, &old, "After")
        .await
        .map_err(|e| format!("rename: {e:?}"))?;
    let again = t
        .provider
        .ensure_label(&t.ctx, "After")
        .await
        .map_err(|e| format!("ensure_label: {e:?}"))?;
    if again != old {
        return Err("renamed label is not found under its new name".into());
    }
    let other = t
        .provider
        .ensure_label(&t.ctx, "Other")
        .await
        .map_err(|e| format!("ensure_label: {e:?}"))?;
    match t.provider.rename_label(&t.ctx, &other, "after").await {
        Err(ports::MailError::Invalid(reason)) if reason == "label_exists" => {}
        other => return Err(format!("clash should be label_exists, got {other:?}")),
    }
    match t.provider.rename_label(&t.ctx, "Label_9999", "Nope").await {
        Err(ports::MailError::NotFound) => {}
        other => return Err(format!("missing label should be NotFound, got {other:?}")),
    }
    Ok(())
}

/// INV-5: `remove_label` removes the label and keeps every message. `t` is a
/// fresh mailbox.
///
/// # Errors
///
/// Returns `Err` describing the first wrong result.
pub async fn remove_label_keeps_messages(t: &MailTarget) -> Result<(), String> {
    let a = t.seeder.seed(&seed("a", "a", 100, &["INBOX"])).await?;
    let b = t.seeder.seed(&seed("b", "b", 200, &["INBOX"])).await?;
    let label = t
        .provider
        .ensure_label(&t.ctx, "Doomed")
        .await
        .map_err(|e| format!("ensure_label: {e:?}"))?;
    let add = LabelSet::from_ids(vec![label.clone()]);
    for id in [&a, &b] {
        t.provider
            .set_labels(&t.ctx, id, &add, &LabelSet::new())
            .await
            .map_err(|e| format!("set_labels: {e:?}"))?;
    }
    t.provider
        .remove_label(&t.ctx, &label)
        .await
        .map_err(|e| format!("remove_label: {e:?}"))?;
    for id in [&a, &b] {
        let meta = t
            .provider
            .get_meta(&t.ctx, id)
            .await
            .map_err(|e| format!("message gone after remove_label: {e:?}"))?;
        if meta.labels.contains(&label) {
            return Err("message still carries the removed label".into());
        }
        if !meta.labels.contains("INBOX") {
            return Err("message lost its other labels".into());
        }
    }
    let inbox = t
        .provider
        .inbox_count(&t.ctx)
        .await
        .map_err(|e| format!("inbox_count: {e:?}"))?;
    if inbox != 2 {
        return Err(format!("inbox count {inbox} after remove_label, want 2"));
    }
    match t.provider.remove_label(&t.ctx, &label).await {
        Err(ports::MailError::NotFound) => {}
        other => return Err(format!("second removal should be NotFound, got {other:?}")),
    }
    if t.seeder.permanent_delete_attempts().await? != 0 {
        return Err("remove_label attempted a permanent delete".into());
    }
    Ok(())
}
fn _unused(_: &ListOrder) {}
