//! The shared `MailProvider` contract suite.

use std::sync::Arc;

use async_trait::async_trait;
use domain::{LabelSet, MessageId};
use ports::{ListOrder, MailProvider, MailboxCtx, MessagePage, PageToken};

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

        // A trashed message is not listed.
        {
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
    }

    if groups.modify {
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

fn _unused(_: &ListOrder) {}
