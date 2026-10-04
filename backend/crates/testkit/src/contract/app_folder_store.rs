//! The shared `AppFolderStore` contract suite.

use std::sync::Arc;

use async_trait::async_trait;
use ports::{AppFolderStore, ETag, MailboxCtx};

/// A control handle to simulate the user deleting the app file.
#[async_trait]
pub trait AppFolderControl: Send + Sync {
    async fn user_deletes_file(&self) -> Result<(), String>;
}

/// A target under test.
pub struct AppFolderTarget {
    pub store: Arc<dyn AppFolderStore>,
    pub ctx: MailboxCtx,
    pub control: Arc<dyn AppFolderControl>,
}

/// Each call to `make` returns a fresh, empty app folder.
///
/// # Errors
///
/// Returns `Err` with the name of the first failing case.
pub async fn app_folder_store<F, Fut>(make: F) -> Result<(), String>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = AppFolderTarget>,
{
    // Empty read is None.
    {
        let t = make().await;
        let read = t
            .store
            .read(&t.ctx)
            .await
            .map_err(|e| format!("read: {e:?}"))?;
        if read.is_some() {
            return Err("empty folder read should be None".into());
        }
    }

    // write(None) creates and returns an ETag; second write(None) is Conflict.
    {
        let t = make().await;
        let tag = t
            .store
            .write(&t.ctx, b"a", None)
            .await
            .map_err(|e| format!("write: {e:?}"))?;
        match t.store.write(&t.ctx, b"b", None).await {
            Err(ports::AppFolderError::Conflict) => {}
            _ => return Err("second write(None) should Conflict".into()),
        }
        // write(Some(current)) succeeds and changes the ETag.
        let tag2 = t
            .store
            .write(&t.ctx, b"c", Some(&tag))
            .await
            .map_err(|e| format!("write: {e:?}"))?;
        if tag2 == tag {
            return Err("write should change the ETag".into());
        }
        // write(Some(stale)) is Conflict and content unchanged.
        match t.store.write(&t.ctx, b"d", Some(&tag)).await {
            Err(ports::AppFolderError::Conflict) => {}
            _ => return Err("write(stale) should Conflict".into()),
        }
        let read = t
            .store
            .read(&t.ctx)
            .await
            .map_err(|e| format!("read: {e:?}"))?;
        if read.as_ref().map(|(bytes, _)| bytes.as_slice()) != Some(&b"c"[..]) {
            return Err("content should be unchanged after stale write".into());
        }
    }

    // user_deletes_file -> read None, write(Some(old)) Conflict.
    {
        let t = make().await;
        let tag = t
            .store
            .write(&t.ctx, b"a", None)
            .await
            .map_err(|e| format!("write: {e:?}"))?;
        t.control.user_deletes_file().await?;
        let read = t
            .store
            .read(&t.ctx)
            .await
            .map_err(|e| format!("read: {e:?}"))?;
        if read.is_some() {
            return Err("read after delete should be None".into());
        }
        match t.store.write(&t.ctx, b"b", Some(&tag)).await {
            Err(ports::AppFolderError::Conflict) => {}
            _ => return Err("write(Some(old)) after delete should Conflict".into()),
        }
    }

    // delete twice is Ok.
    {
        let t = make().await;
        let _ = t
            .store
            .write(&t.ctx, b"a", None)
            .await
            .map_err(|e| format!("write: {e:?}"))?;
        t.store
            .delete(&t.ctx)
            .await
            .map_err(|e| format!("delete: {e:?}"))?;
        t.store
            .delete(&t.ctx)
            .await
            .map_err(|e| format!("delete 2: {e:?}"))?;
    }

    Ok(())
}

fn _t(_: &ETag) {}
