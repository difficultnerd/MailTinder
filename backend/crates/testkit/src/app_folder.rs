//! An in-memory app folder fake.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Mutex;

use async_trait::async_trait;
use domain::MailboxId;
use ports::{AppFolderError, AppFolderStore, ETag, MailError, MailboxCtx};

/// An in-memory app folder, one file per mailbox.
pub struct InMemoryAppFolder {
    files: Mutex<HashMap<MailboxId, (Vec<u8>, u64)>>,
    fail_next: AtomicU32,
}

impl InMemoryAppFolder {
    pub fn new() -> Self {
        Self {
            files: Mutex::new(HashMap::new()),
            fail_next: AtomicU32::new(0),
        }
    }

    pub fn user_deleted_file(&self, mb: &MailboxId) {
        self.files
            .lock()
            .unwrap_or_else(|_| panic!("app folder poisoned"))
            .remove(mb);
    }

    pub fn fail_next(&self, n: u32) {
        self.fail_next.store(n, Ordering::SeqCst);
    }

    fn arm_check(&self) -> Result<(), MailError> {
        let mut cur = self.fail_next.load(Ordering::SeqCst);
        loop {
            if cur == 0 {
                return Ok(());
            }
            match self
                .fail_next
                .compare_exchange(cur, cur - 1, Ordering::SeqCst, Ordering::SeqCst)
            {
                Ok(_) => return Err(MailError::Transient),
                Err(actual) => cur = actual,
            }
        }
    }
}

impl Default for InMemoryAppFolder {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl AppFolderStore for InMemoryAppFolder {
    async fn read(&self, mb: &MailboxCtx) -> Result<Option<(Vec<u8>, ETag)>, MailError> {
        self.arm_check()?;
        let files = self
            .files
            .lock()
            .unwrap_or_else(|_| panic!("app folder poisoned"));
        Ok(files
            .get(&mb.mailbox)
            .map(|(bytes, tag)| (bytes.clone(), ETag(tag.to_string()))))
    }

    async fn write(
        &self,
        mb: &MailboxCtx,
        bytes: &[u8],
        if_match: Option<&ETag>,
    ) -> Result<ETag, AppFolderError> {
        self.arm_check()?;
        let mut files = self
            .files
            .lock()
            .unwrap_or_else(|_| panic!("app folder poisoned"));
        let current = files.get(&mb.mailbox).map(|(_, tag)| tag.to_string());
        match if_match {
            None => {
                if current.is_some() {
                    return Err(AppFolderError::Conflict);
                }
            }
            Some(tag) => {
                if current.as_deref() != Some(tag.0.as_str()) {
                    return Err(AppFolderError::Conflict);
                }
            }
        }
        let next = files.get(&mb.mailbox).map_or(1, |(_, t)| t + 1);
        files.insert(mb.mailbox, (bytes.to_vec(), next));
        Ok(ETag(next.to_string()))
    }

    async fn delete(&self, mb: &MailboxCtx) -> Result<(), MailError> {
        self.arm_check()?;
        self.files
            .lock()
            .unwrap_or_else(|_| panic!("app folder poisoned"))
            .remove(&mb.mailbox);
        Ok(())
    }
}
