//! The real `AppFolderStore`: Google Drive's hidden `appDataFolder` (T-405).
//!
//! One ciphertext file per user (`APP_FILE_NAME`) in the primary mailbox's
//! Drive. Every write carries an `If-Match` precondition so two tabs cannot
//! overwrite each other's History; the api owns the re-read-and-retry loop on
//! [`AppFolderError::Conflict`]. This module never encrypts or decrypts: it
//! moves bytes, and the caller encrypts under the user's `data_key`.
//!
//! Scope is `drive.appdata` only. The four [`DRIVE_V3`]-style constants are the
//! production API roots; the client's injected base replaces the host so
//! `fake-google` and production share one code path.
//!
//! This is the only source file in the crate allowed to issue a `DELETE`
//! (T-403's structural test exempts it): it deletes our own app-data file, not
//! a message, so INV-5 is untouched.

use async_trait::async_trait;
use obs::{op_log, OpLog};
use ports::{AppFolderError, AppFolderStore, ETag, HttpMethod, MailError, MailboxCtx};
use serde::Deserialize;
use url::Url;

use crate::client::GmailHttp;

/// The production host every Drive call shares.
pub const GOOGLEAPIS_ROOT: &str = "https://www.googleapis.com";
/// The Drive v3 API root.
pub const DRIVE_V3: &str = "https://www.googleapis.com/drive/v3";
/// The Drive v2 API root, used for `etag` and the `If-Match` update.
pub const DRIVE_V2: &str = "https://www.googleapis.com/drive/v2";
/// The Drive v3 upload root.
pub const DRIVE_UPLOAD_V3: &str = "https://www.googleapis.com/upload/drive/v3";
/// The Drive v2 upload root.
pub const DRIVE_UPLOAD_V2: &str = "https://www.googleapis.com/upload/drive/v2";
/// The one app folder file name `[DEFAULT]`.
pub const APP_FILE_NAME: &str = "mailtinder-state-v1.bin";
/// The simple-upload size limit `[DEFAULT]`; History is trimmed at 12 months (S5).
pub const APP_FILE_MAX_BYTES: usize = 5 * 1024 * 1024;

/// The multipart boundary for the create call. Fixed so the request is
/// deterministic; the JSON part and the ciphertext are separated by it.
const MULTIPART_BOUNDARY: &str = "mailtinder-app-folder-7f3a91c4";

/// The app folder store over Google Drive.
pub struct DriveAppFolder {
    http: GmailHttp,
}

impl DriveAppFolder {
    /// Build a store over the given client. `http`'s base is the API host (the
    /// `fake-google` base in tests, [`GOOGLEAPIS_ROOT`] in production).
    pub fn new(http: GmailHttp) -> Self {
        Self { http }
    }

    /// The id of the app folder file, or `None` when absent. Two raced creates
    /// leave duplicates; the newest `modifiedTime` wins and the count is logged.
    async fn find(&self, mb: &MailboxCtx) -> Result<Option<String>, MailError> {
        let q = format!("name = '{APP_FILE_NAME}' and trashed = false");
        let url = self.api_url(
            DRIVE_V3,
            "/files",
            &[
                ("spaces", "appDataFolder".to_owned()),
                ("q", q),
                ("fields", "files(id,modifiedTime)".to_owned()),
                ("pageSize", "10".to_owned()),
            ],
        );
        let resp = self
            .http
            .call_raw(mb, HttpMethod::Get, url, &[], None, None)
            .await?;
        if !(200..300).contains(&resp.status) {
            return Err(self.http.map_response(&resp));
        }
        let list: FileList =
            serde_json::from_slice(&resp.body).map_err(|_| MailError::Transient)?;
        match list.files.len() {
            0 => Ok(None),
            1 => Ok(Some(list.files[0].id.clone())),
            n => {
                // Duplicates from a create race: keep the newest, log only the count.
                op_log(&OpLog {
                    op: "drive.files.list",
                    outcome: "app_folder_duplicates",
                    status: u16::try_from(n).ok(),
                    latency_ms: None,
                });
                let newest = list
                    .files
                    .iter()
                    .max_by(|a, b| a.modified_time.cmp(&b.modified_time))
                    .or_else(|| list.files.first())
                    .ok_or(MailError::Transient)?;
                Ok(Some(newest.id.clone()))
            }
        }
    }

    /// The v2 `etag` for a file id. `NotFound` lets `read` treat a file deleted
    /// mid-flight as absent.
    async fn etag(&self, mb: &MailboxCtx, id: &str) -> Result<String, MailError> {
        let url = self.api_url(
            DRIVE_V2,
            &format!("/files/{id}"),
            &[("fields", "id,etag".to_owned())],
        );
        let resp = self
            .http
            .call_raw(mb, HttpMethod::Get, url, &[], None, None)
            .await?;
        if !(200..300).contains(&resp.status) {
            return Err(self.http.map_response(&resp));
        }
        let file: V2File = serde_json::from_slice(&resp.body).map_err(|_| MailError::Transient)?;
        Ok(file.etag)
    }

    /// Create the file (multipart) and return its id.
    async fn create(&self, mb: &MailboxCtx, bytes: &[u8]) -> Result<String, MailError> {
        let url = self.api_url(
            DRIVE_UPLOAD_V3,
            "/files",
            &[
                ("uploadType", "multipart".to_owned()),
                ("fields", "id".to_owned()),
            ],
        );
        let meta = format!("{{\"name\":\"{APP_FILE_NAME}\",\"parents\":[\"appDataFolder\"]}}");
        let body = multipart_body(MULTIPART_BOUNDARY, &meta, bytes);
        let content_type = format!("multipart/related; boundary={MULTIPART_BOUNDARY}");
        let resp = self
            .http
            .call_raw(
                mb,
                HttpMethod::Post,
                url,
                &[],
                Some(body),
                Some(&content_type),
            )
            .await?;
        if !(200..300).contains(&resp.status) {
            return Err(self.http.map_response(&resp));
        }
        let created: CreatedFile =
            serde_json::from_slice(&resp.body).map_err(|_| MailError::Transient)?;
        Ok(created.id)
    }

    /// Replace the bytes under an `If-Match` precondition; a `412` is a
    /// [`AppFolderError::Conflict`].
    async fn update(
        &self,
        mb: &MailboxCtx,
        id: &str,
        bytes: &[u8],
        if_match: &ETag,
    ) -> Result<ETag, AppFolderError> {
        let url = self.api_url(
            DRIVE_UPLOAD_V2,
            &format!("/files/{id}"),
            &[
                ("uploadType", "media".to_owned()),
                ("fields", "id,etag".to_owned()),
            ],
        );
        let headers = [("If-Match".to_owned(), if_match.0.clone())];
        let resp = self
            .http
            .call_raw(
                mb,
                HttpMethod::Put,
                url,
                &headers,
                Some(bytes.to_vec()),
                Some("application/octet-stream"),
            )
            .await
            .map_err(AppFolderError::Mail)?;
        if resp.status == 412 {
            return Err(AppFolderError::Conflict);
        }
        if !(200..300).contains(&resp.status) {
            return Err(AppFolderError::Mail(self.http.map_response(&resp)));
        }
        let file: V2File = serde_json::from_slice(&resp.body)
            .map_err(|_| AppFolderError::Mail(MailError::Transient))?;
        Ok(ETag(file.etag))
    }

    /// Join a Drive API root and path onto the client's base, with the query
    /// appended by the URL encoder (the `q` string has spaces and quotes).
    fn api_url(&self, api_root: &str, tail: &str, query: &[(&str, String)]) -> Url {
        let rel = api_root.strip_prefix(GOOGLEAPIS_ROOT).unwrap_or(api_root);
        let mut url = self.http.base().clone();
        let prefix = url.path().trim_end_matches('/').to_owned();
        url.set_path(&format!("{prefix}{rel}{tail}"));
        url.set_query(None);
        {
            let mut pairs = url.query_pairs_mut();
            for (key, value) in query {
                pairs.append_pair(key, value);
            }
        }
        url
    }
}

#[async_trait]
impl AppFolderStore for DriveAppFolder {
    async fn read(&self, mb: &MailboxCtx) -> Result<Option<(Vec<u8>, ETag)>, MailError> {
        let Some(id) = self.find(mb).await? else {
            return Ok(None);
        };
        // Read the ETag before the bytes: a write between the two then makes
        // the next write fail with Conflict rather than overwrite unseen data.
        let etag = match self.etag(mb, &id).await {
            Ok(tag) => tag,
            Err(MailError::NotFound) => return Ok(None),
            Err(e) => return Err(e),
        };
        let url = self.api_url(
            DRIVE_V3,
            &format!("/files/{id}"),
            &[("alt", "media".to_owned())],
        );
        let resp = self
            .http
            .call_raw(mb, HttpMethod::Get, url, &[], None, None)
            .await?;
        match resp.status {
            404 | 410 => Ok(None),
            status if (200..300).contains(&status) => {
                if resp.body.len() > APP_FILE_MAX_BYTES {
                    return Err(MailError::Invalid("app_folder_too_large".to_owned()));
                }
                Ok(Some((resp.body, ETag(etag))))
            }
            _ => Err(self.http.map_response(&resp)),
        }
    }

    async fn write(
        &self,
        mb: &MailboxCtx,
        bytes: &[u8],
        if_match: Option<&ETag>,
    ) -> Result<ETag, AppFolderError> {
        if bytes.len() > APP_FILE_MAX_BYTES {
            return Err(AppFolderError::Mail(MailError::Invalid(
                "app_folder_too_large".to_owned(),
            )));
        }
        match if_match {
            None => {
                if self.find(mb).await.map_err(AppFolderError::Mail)?.is_some() {
                    return Err(AppFolderError::Conflict);
                }
                let id = self.create(mb, bytes).await.map_err(AppFolderError::Mail)?;
                let tag = self.etag(mb, &id).await.map_err(AppFolderError::Mail)?;
                Ok(ETag(tag))
            }
            Some(tag) => {
                let Some(id) = self.find(mb).await.map_err(AppFolderError::Mail)? else {
                    // The file was deleted; the caller re-reads and starts again.
                    return Err(AppFolderError::Conflict);
                };
                self.update(mb, &id, bytes, tag).await
            }
        }
    }

    async fn delete(&self, mb: &MailboxCtx) -> Result<(), MailError> {
        let Some(id) = self.find(mb).await? else {
            return Ok(());
        };
        let url = self.api_url(DRIVE_V3, &format!("/files/{id}"), &[]);
        let resp = self
            .http
            .call_raw(mb, HttpMethod::Delete, url, &[], None, None)
            .await?;
        match resp.status {
            404 | 410 => Ok(()),
            status if (200..300).contains(&status) => Ok(()),
            _ => Err(self.http.map_response(&resp)),
        }
    }
}

/// A `files.list` response body.
#[derive(Deserialize)]
struct FileList {
    #[serde(default)]
    files: Vec<FileRef>,
}

/// One entry of a `files.list` response.
#[derive(Deserialize)]
struct FileRef {
    id: String,
    #[serde(rename = "modifiedTime")]
    modified_time: Option<String>,
}

/// A v2 file resource (metadata get and update response).
#[derive(Deserialize)]
struct V2File {
    #[allow(dead_code)]
    id: String,
    etag: String,
}

/// The `files.create` response body.
#[derive(Deserialize)]
struct CreatedFile {
    id: String,
}

/// Build a `multipart/related` body: a JSON metadata part then the bytes.
fn multipart_body(boundary: &str, meta_json: &str, data: &[u8]) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Type: application/json; charset=UTF-8\r\n\r\n{meta_json}\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(
        format!("--{boundary}\r\nContent-Type: application/octet-stream\r\n\r\n").as_bytes(),
    );
    body.extend_from_slice(data);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    body
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use std::sync::Arc;

    use domain::MailboxId;
    use obs::Sensitive;
    use ports::{Clock, EgressError, EgressRequest, EgressResponse, HttpEgress, OneClickOutcome};

    use super::*;

    /// An egress that refuses everything: the size guard must refuse before
    /// any call reaches the network.
    struct NoEgress;

    #[async_trait]
    impl HttpEgress for NoEgress {
        async fn one_click_post(&self, _url: &Url) -> Result<OneClickOutcome, EgressError> {
            Err(EgressError::HostNotAllowed)
        }

        async fn call(&self, _req: EgressRequest) -> Result<EgressResponse, EgressError> {
            Err(EgressError::HostNotAllowed)
        }
    }

    struct FixedClock;

    impl Clock for FixedClock {
        fn now(&self) -> time::OffsetDateTime {
            time::macros::datetime!(2026-10-05 00:00 UTC)
        }
    }

    fn store() -> DriveAppFolder {
        let base = Url::parse("https://www.googleapis.com/").expect("base url");
        DriveAppFolder::new(GmailHttp::new(
            Arc::new(NoEgress),
            base,
            Arc::new(FixedClock),
        ))
    }

    fn ctx() -> MailboxCtx {
        MailboxCtx {
            mailbox: MailboxId(uuid::Uuid::from_u128(1)),
            access_token: Sensitive::new("tok".to_owned()),
        }
    }

    #[tokio::test]
    async fn app_folder_too_large_refused() {
        let store = store();
        let big = vec![0u8; APP_FILE_MAX_BYTES + 1];
        match store.write(&ctx(), &big, None).await {
            Err(AppFolderError::Mail(MailError::Invalid(reason))) => {
                assert_eq!(reason, "app_folder_too_large");
            }
            other => panic!("expected app_folder_too_large, got {other:?}"),
        }
    }

    /// The S8 contract constants are the production URLs.
    #[test]
    fn drive_constants_are_the_googleapis_urls() {
        assert_eq!(DRIVE_V3, "https://www.googleapis.com/drive/v3");
        assert_eq!(DRIVE_V2, "https://www.googleapis.com/drive/v2");
        assert_eq!(
            DRIVE_UPLOAD_V3,
            "https://www.googleapis.com/upload/drive/v3"
        );
        assert_eq!(
            DRIVE_UPLOAD_V2,
            "https://www.googleapis.com/upload/drive/v2"
        );
    }
}
