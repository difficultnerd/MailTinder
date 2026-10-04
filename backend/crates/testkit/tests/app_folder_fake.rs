//! Run the app folder contract suite against `InMemoryAppFolder`.

use std::sync::Arc;

use async_trait::async_trait;
use domain::MailboxId;
use ports::{AppFolderStore, MailboxCtx};
use testkit::contract::app_folder_store::{app_folder_store, AppFolderControl, AppFolderTarget};
use testkit::InMemoryAppFolder;

const MAILBOX: MailboxId = MailboxId(uuid::Uuid::from_u128(0));

struct Control(Arc<InMemoryAppFolder>, MailboxId);

#[async_trait]
impl AppFolderControl for Control {
    async fn user_deletes_file(&self) -> Result<(), String> {
        self.0.user_deleted_file(&self.1);
        Ok(())
    }
}

#[tokio::test]
async fn app_folder_store_contract_in_memory() {
    let result = app_folder_store(|| async {
        let fake = Arc::new(InMemoryAppFolder::new());
        AppFolderTarget {
            store: Arc::clone(&fake) as Arc<dyn AppFolderStore>,
            ctx: MailboxCtx {
                mailbox: MAILBOX,
                access_token: obs::Sensitive::new("tok".to_owned()),
            },
            control: Arc::new(Control(fake, MAILBOX)),
        }
    })
    .await;
    assert_eq!(result, Ok(()), "contract failed: {result:?}");
}
