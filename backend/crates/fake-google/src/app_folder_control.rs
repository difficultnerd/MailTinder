//! `AppFolderControl` for `fake-google` (T-205b): lets the contract suite
//! simulate the user deleting the app-data file.

use std::sync::Arc;

use async_trait::async_trait;
use testkit::contract::app_folder_store::AppFolderControl;

use super::state::FakeMailboxKey;
use super::state::FakeState;

/// A control handle that deletes every app-data file for one mailbox.
pub struct FakeAppFolderControl {
    state: Arc<std::sync::Mutex<FakeState>>,
    mb: FakeMailboxKey,
}

impl FakeAppFolderControl {
    pub fn new(state: Arc<std::sync::Mutex<FakeState>>, mb: FakeMailboxKey) -> Self {
        Self { state, mb }
    }
}

#[async_trait]
impl AppFolderControl for FakeAppFolderControl {
    async fn user_deletes_file(&self) -> Result<(), String> {
        let mut st = self.state.lock().unwrap();
        if let Some(m) = st.mailboxes.get_mut(&self.mb.0) {
            m.drive.files.clear();
        }
        Ok(())
    }
}
