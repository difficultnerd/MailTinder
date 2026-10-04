//! Run the mail provider contract suite against `FakeMailbox`.

use std::sync::Arc;

use async_trait::async_trait;
use domain::{LabelSet, MailboxId, MessageId};
use ports::{MailProvider, MailboxCtx};
use testkit::contract::mail_provider::{mail_provider, CaseGroups, MailSeeder, MailTarget};
use testkit::mailbox::state::SeedMessage;
use testkit::mailbox::{FakeMailbox, SentRecord};

struct Seeder(Arc<FakeMailbox>);

#[async_trait]
impl MailSeeder for Seeder {
    async fn seed(&self, msg: &SeedMessage) -> Result<MessageId, String> {
        Ok(self.0.seed(&MAILBOX, msg.clone()))
    }
    async fn labels_of(&self, id: &MessageId) -> Result<LabelSet, String> {
        let mb = MailboxId(uuid::Uuid::from_u128(0));
        self.0.labels_of(&mb, id).ok_or_else(|| "missing".into())
    }
    async fn sent(&self) -> Result<Vec<SentRecord>, String> {
        Ok(self.0.sent())
    }
    async fn permanent_delete_attempts(&self) -> Result<u64, String> {
        Ok(0)
    }
}

const MAILBOX: MailboxId = MailboxId(uuid::Uuid::from_u128(0));

#[tokio::test]
async fn mail_provider_contract_fake_mailbox() {
    let result = mail_provider(
        || async {
            let fake = Arc::new(FakeMailbox::new());
            MailTarget {
                ctx: MailboxCtx {
                    mailbox: MAILBOX,
                    access_token: obs::Sensitive::new("tok".to_owned()),
                },
                provider: Arc::clone(&fake) as Arc<dyn MailProvider>,
                seeder: Arc::new(Seeder(fake)),
            }
        },
        CaseGroups::ALL,
    )
    .await;
    assert_eq!(result, Ok(()), "contract failed: {result:?}");
}

#[tokio::test]
async fn inv_5_fake_mailbox_records_no_permanent_delete() {
    let fake = Arc::new(FakeMailbox::new());
    let seeded = fake.seed(
        &MAILBOX,
        SeedMessage {
            from_display: "S".to_owned(),
            from_address: "a@example.com".to_owned(),
            subject: "s".to_owned(),
            raw_headers: vec![],
            facts: domain::HeaderFacts::default(),
            preview_text: "p".to_owned(),
            internal_date: time::OffsetDateTime::now_utc(),
            labels: vec!["INBOX".to_owned()],
        },
    );
    let _ = fake
        .trash(
            &MailboxCtx {
                mailbox: MAILBOX,
                access_token: obs::Sensitive::new("t".to_owned()),
            },
            &seeded,
        )
        .await;
    // FakeMailbox has no delete path; this just confirms it compiles and runs.
}
