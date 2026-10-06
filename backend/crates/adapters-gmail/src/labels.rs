//! Label definition changes (T-602a): rename and remove.
//!
//! Removing a label deletes the label definition only; Gmail detaches it from
//! its messages and never removes a message (INV-5). The label ID goes into a
//! URL path, so it is validated first.

use ports::{MailError, MailboxCtx};
use serde::{Deserialize, Serialize};

use crate::messages::validate_label_id;
use crate::read::GmailProvider;

#[derive(Serialize)]
struct RenameLabel<'a> {
    name: &'a str,
}

#[derive(Deserialize)]
struct Renamed {}

impl GmailProvider {
    /// `labels.patch` with the new name. A name clash is `Invalid("label_exists")`.
    pub(crate) async fn rename_label_named(
        &self,
        mb: &MailboxCtx,
        label_id: &str,
        new_name: &str,
    ) -> Result<(), MailError> {
        let label_id = validate_label_id(label_id)?;
        let path = format!("labels/{label_id}");
        let query = vec![("fields", "id".to_owned())];
        let body = RenameLabel { name: new_name };
        let result: Result<Renamed, MailError> =
            self.http().patch_json(mb, &path, &query, &body).await;
        match result {
            Ok(_) => Ok(()),
            Err(MailError::Invalid(reason)) if reason == "gmail_conflict" => {
                Err(MailError::Invalid("label_exists".to_owned()))
            }
            Err(other) => Err(other),
        }
    }

    /// The label endpoint's remove call: the definition goes, the messages stay.
    pub(crate) async fn remove_label_by_id(
        &self,
        mb: &MailboxCtx,
        label_id: &str,
    ) -> Result<(), MailError> {
        let label_id = validate_label_id(label_id)?;
        let path = format!("labels/{label_id}");
        self.http().remove_empty(mb, &path).await
    }
}
