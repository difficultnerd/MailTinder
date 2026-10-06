//! An in-memory app folder fake.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Mutex;

use async_trait::async_trait;
use domain::MailboxId;
use ports::{AppFolderError, AppFolderStore, ETag, MailError, MailboxCtx};

/// An app folder operation, for a targeted armed failure (T-601b tests).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FolderOp {
    Read,
    Write,
    Delete,
}

/// An in-memory app folder, one file per mailbox.
pub struct InMemoryAppFolder {
    files: Mutex<HashMap<MailboxId, (Vec<u8>, u64)>>,
    fail_next: AtomicU32,
    fail_read: AtomicU32,
    fail_write: AtomicU32,
    fail_delete: AtomicU32,
}

impl InMemoryAppFolder {
    pub fn new() -> Self {
        Self {
            files: Mutex::new(HashMap::new()),
            fail_next: AtomicU32::new(0),
            fail_read: AtomicU32::new(0),
            fail_write: AtomicU32::new(0),
            fail_delete: AtomicU32::new(0),
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

    /// Make the next `n` calls of one operation fail with
    /// `MailError::Transient`, leaving the others working.
    pub fn fail_op(&self, op: FolderOp, n: u32) {
        let counter = match op {
            FolderOp::Read => &self.fail_read,
            FolderOp::Write => &self.fail_write,
            FolderOp::Delete => &self.fail_delete,
        };
        counter.store(n, Ordering::SeqCst);
    }

    fn arm_check(&self, op: FolderOp) -> Result<(), MailError> {
        let targeted = match op {
            FolderOp::Read => &self.fail_read,
            FolderOp::Write => &self.fail_write,
            FolderOp::Delete => &self.fail_delete,
        };
        for counter in [targeted, &self.fail_next] {
            let mut cur = counter.load(Ordering::SeqCst);
            loop {
                if cur == 0 {
                    break;
                }
                match counter.compare_exchange(cur, cur - 1, Ordering::SeqCst, Ordering::SeqCst) {
                    Ok(_) => return Err(MailError::Transient),
                    Err(actual) => cur = actual,
                }
            }
        }
        Ok(())
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
        self.arm_check(FolderOp::Read)?;
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
        self.arm_check(FolderOp::Write)?;
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
        self.arm_check(FolderOp::Delete)?;
        self.files
            .lock()
            .unwrap_or_else(|_| panic!("app folder poisoned"))
            .remove(&mb.mailbox);
        Ok(())
    }
}
