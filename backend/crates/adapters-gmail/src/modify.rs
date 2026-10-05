//! The mailbox-changing half of [`MailProvider`] over the Gmail REST API.
//!
//! Every change returns or accepts an exact [`LabelSet`], so an undo puts back
//! the exact previous labels (S3 "Swipe and undo"), and no method can
//! permanently delete a message (INV-5). The send path lives in
//! [`crate::send`] (T-404). All bodies are `application/json`; base, headers,
//! timeout and error mapping are the ones in [`crate::client`] (T-401).

use domain::{LabelSet, MessageId};
use ports::{MailError, MailboxCtx};
use serde::{Deserialize, Serialize};

use crate::read::GmailProvider;

/// The user label the unsubscribe mail is filed under (S2 UN-03 AC2, T-404).
pub const MAIL_TINDER_LABEL: &str = "Mail Tinder";
/// The Gmail label-name limit.
pub const LABEL_NAME_MAX_CHARS: usize = 225;
/// System labels Gmail does not let `messages.modify` add or remove.
pub const IMMUTABLE_LABELS: [&str; 3] = ["SENT", "DRAFT", "CHAT"];

/// The trash system label.
const TRASH: &str = "TRASH";
/// The spam system label.
const SPAM: &str = "SPAM";
/// The `INBOX` system label.
const INBOX: &str = "INBOX";
/// Gmail's prefix for user label IDs (`Label_123`), never a name.
const USER_LABEL_PREFIX: &str = "Label_";
/// The system label names a user may not reuse (case-insensitive).
const SYSTEM_LABEL_NAMES: [&str; 9] = [
    "INBOX",
    "SPAM",
    "TRASH",
    "UNREAD",
    "STARRED",
    "IMPORTANT",
    "SENT",
    "DRAFT",
    "CHAT",
];

#[derive(Serialize)]
struct ModifyRequest<'a> {
    #[serde(rename = "addLabelIds")]
    add: Vec<&'a str>,
    #[serde(rename = "removeLabelIds")]
    remove: Vec<&'a str>,
}

#[derive(Deserialize)]
struct LabelsOnly {
    #[serde(rename = "labelIds", default)]
    label_ids: Vec<String>,
}

#[derive(Deserialize)]
struct LabelList {
    #[serde(default)]
    labels: Vec<LabelItem>,
}

#[derive(Deserialize)]
struct LabelItem {
    id: String,
    name: String,
    #[allow(dead_code)] // part of the wire shape; the adapter never reads it.
    #[serde(rename = "type")]
    kind: Option<String>,
}

#[derive(Serialize)]
struct CreateLabel<'a> {
    name: &'a str,
    #[serde(rename = "labelListVisibility")]
    list_vis: &'static str,
    #[serde(rename = "messageListVisibility")]
    msg_vis: &'static str,
}

#[derive(Deserialize)]
struct CreatedLabel {
    id: String,
}

impl GmailProvider {
    /// `messages.get format=minimal`: the labels a message carries right now.
    pub(crate) async fn current_labels(
        &self,
        mb: &MailboxCtx,
        id: &MessageId,
    ) -> Result<LabelSet, MailError> {
        let query = vec![
            ("format", "minimal".to_owned()),
            ("fields", "labelIds".to_owned()),
        ];
        let path = format!("messages/{}", id.as_str());
        let body: LabelsOnly = self.http().get_json(mb, &path, &query).await?;
        Ok(LabelSet::from_ids(body.label_ids))
    }

    /// `messages.modify`: add and remove labels, returning the new set.
    ///
    /// `remove` wins over `add` where the two overlap (S8 `[DEFAULT]`).
    pub(crate) async fn modify_labels(
        &self,
        mb: &MailboxCtx,
        id: &MessageId,
        add: &LabelSet,
        remove: &LabelSet,
    ) -> Result<LabelSet, MailError> {
        refuse_forbidden(add)?;
        refuse_forbidden(remove)?;
        let add_ids: Vec<&str> = add
            .iter()
            .map(String::as_str)
            .filter(|l| !remove.contains(l))
            .collect();
        if add_ids.is_empty() && remove.is_empty() {
            return self.current_labels(mb, id).await;
        }
        let remove_ids: Vec<&str> = remove.iter().map(String::as_str).collect();
        let path = format!("messages/{}/modify", id.as_str());
        let query = vec![("fields", "labelIds".to_owned())];
        let body = ModifyRequest {
            add: add_ids,
            remove: remove_ids,
        };
        let out: LabelsOnly = self.http().post_json(mb, &path, &query, &body).await?;
        Ok(LabelSet::from_ids(out.label_ids))
    }

    /// `messages.trash`: returns the labels as they were *before* the trash,
    /// which is the state an undo must restore.
    pub(crate) async fn trash_message(
        &self,
        mb: &MailboxCtx,
        id: &MessageId,
    ) -> Result<LabelSet, MailError> {
        let before = self.current_labels(mb, id).await?;
        let path = format!("messages/{}/trash", id.as_str());
        let query = vec![("fields", "labelIds".to_owned())];
        let _after: LabelsOnly = self.http().post_json_empty(mb, &path, &query).await?;
        Ok(before)
    }

    /// `messages.modify` with `add SPAM`, `remove INBOX`: Gmail trains its spam
    /// filter on the added label. Returns the labels before the report.
    /// Whether the swipe also trashes is T-605's decision, not this adapter's.
    pub(crate) async fn report_spam_message(
        &self,
        mb: &MailboxCtx,
        id: &MessageId,
    ) -> Result<LabelSet, MailError> {
        let before = self.current_labels(mb, id).await?;
        let path = format!("messages/{}/modify", id.as_str());
        let query = vec![("fields", "labelIds".to_owned())];
        let body = ModifyRequest {
            add: vec![SPAM],
            remove: vec![INBOX],
        };
        let _after: LabelsOnly = self.http().post_json(mb, &path, &query, &body).await?;
        Ok(before)
    }

    /// Put a message back to exactly `exact`: leave or enter trash, then diff
    /// the labels. A user label Gmail no longer has is dropped rather than
    /// sent to `modify`; a final read proves the restored set (S8).
    pub(crate) async fn restore_labels_exact(
        &self,
        mb: &MailboxCtx,
        id: &MessageId,
        exact: &LabelSet,
    ) -> Result<(), MailError> {
        let mut now = self.current_labels(mb, id).await?;
        if now.contains(TRASH) && !exact.contains(TRASH) {
            let path = format!("messages/{}/untrash", id.as_str());
            let query = vec![("fields", "labelIds".to_owned())];
            let after: LabelsOnly = self.http().post_json_empty(mb, &path, &query).await?;
            now = LabelSet::from_ids(after.label_ids);
        } else if !now.contains(TRASH) && exact.contains(TRASH) {
            let path = format!("messages/{}/trash", id.as_str());
            let query = vec![("fields", "labelIds".to_owned())];
            let _trashed: LabelsOnly = self.http().post_json_empty(mb, &path, &query).await?;
            now = self.current_labels(mb, id).await?;
        }

        let mut add = exact.difference(&now);
        let mut remove = now.difference(exact);
        strip_unrestorable(&mut add);
        strip_unrestorable(&mut remove);

        let mut dropped = LabelSet::new();
        if add.iter().any(|l| l.starts_with(USER_LABEL_PREFIX)) {
            let live = self.list_labels(mb).await?;
            let gone: Vec<String> = add
                .iter()
                .filter(|l| {
                    l.starts_with(USER_LABEL_PREFIX) && !live.iter().any(|item| item.id == **l)
                })
                .cloned()
                .collect();
            for label in gone {
                add.remove(&label);
                dropped.insert(label);
            }
        }

        if !add.is_empty() || !remove.is_empty() {
            let path = format!("messages/{}/modify", id.as_str());
            let query = vec![("fields", "labelIds".to_owned())];
            let body = ModifyRequest {
                add: add.iter().map(String::as_str).collect(),
                remove: remove.iter().map(String::as_str).collect(),
            };
            let _changed: LabelsOnly = self.http().post_json(mb, &path, &query, &body).await?;
        }

        let read = self.current_labels(mb, id).await?;
        let mut expected = exact.clone();
        for label in dropped.iter() {
            expected.remove(label);
        }
        strip_immutable(&mut expected);
        let mut actual = read;
        strip_immutable(&mut actual);
        if actual != expected {
            return Err(MailError::Invalid("restore_mismatch".to_owned()));
        }
        Ok(())
    }

    /// Return the ID of `name`, creating the label first if the mailbox has
    /// none (FL-02 AC1). A `409` means another client created it first.
    pub(crate) async fn ensure_label_named(
        &self,
        mb: &MailboxCtx,
        name: &str,
    ) -> Result<String, MailError> {
        let name = name.trim();
        validate_label_name(name)?;
        if let Some(found) = self.find_label(mb, name).await? {
            return Ok(found);
        }
        let path = "labels";
        let query = vec![("fields", "id,name".to_owned())];
        let body = CreateLabel {
            name,
            list_vis: "labelShow",
            msg_vis: "show",
        };
        match self
            .http()
            .post_json::<CreateLabel<'_>, CreatedLabel>(mb, path, &query, &body)
            .await
        {
            Ok(created) => Ok(created.id),
            // The label appeared between our list and our create (a race).
            Err(MailError::Invalid(reason)) if reason == "gmail_conflict" => {
                self.find_label(mb, name).await?.ok_or(MailError::Transient)
            }
            Err(other) => Err(other),
        }
    }

    /// The `users.labels.list` items for the mailbox.
    async fn list_labels(&self, mb: &MailboxCtx) -> Result<Vec<LabelItem>, MailError> {
        let query = vec![("fields", "labels(id,name,type)".to_owned())];
        let list: LabelList = self.http().get_json(mb, "labels", &query).await?;
        Ok(list.labels)
    }

    /// The ID of the label whose name equals `name` ignoring ASCII case.
    async fn find_label(&self, mb: &MailboxCtx, name: &str) -> Result<Option<String>, MailError> {
        Ok(self
            .list_labels(mb)
            .await?
            .into_iter()
            .find(|item| item.name.eq_ignore_ascii_case(name))
            .map(|item| item.id))
    }
}

/// Refuse a set that carries a label only `trash`/`report_spam` may move.
fn refuse_forbidden(ids: &LabelSet) -> Result<(), MailError> {
    if ids
        .iter()
        .any(|id| id == TRASH || id == SPAM || IMMUTABLE_LABELS.contains(&id.as_str()))
    {
        return Err(MailError::Invalid("label_not_allowed".to_owned()));
    }
    Ok(())
}

/// Drop `TRASH` and the immutable system labels from a change set.
fn strip_unrestorable(ids: &mut LabelSet) {
    let drop: Vec<String> = ids
        .iter()
        .filter(|id| id.as_str() == TRASH || IMMUTABLE_LABELS.contains(&id.as_str()))
        .cloned()
        .collect();
    for id in drop {
        ids.remove(&id);
    }
}

/// Drop the immutable system labels from a comparison set.
fn strip_immutable(ids: &mut LabelSet) {
    let drop: Vec<String> = ids
        .iter()
        .filter(|id| IMMUTABLE_LABELS.contains(&id.as_str()))
        .cloned()
        .collect();
    for id in drop {
        ids.remove(&id);
    }
}

/// A label name must be 1..=225 chars, free of control characters, not start
/// or end with `/`, hold no `//`, and not be a system label name.
fn validate_label_name(name: &str) -> Result<(), MailError> {
    let invalid = || MailError::Invalid("label_name".to_owned());
    let len = name.chars().count();
    if len == 0 || len > LABEL_NAME_MAX_CHARS {
        return Err(invalid());
    }
    if name.chars().any(char::is_control) {
        return Err(invalid());
    }
    if name.starts_with('/') || name.ends_with('/') || name.contains("//") {
        return Err(invalid());
    }
    if SYSTEM_LABEL_NAMES
        .iter()
        .any(|system| system.eq_ignore_ascii_case(name))
    {
        return Err(invalid());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    fn set(ids: &[&str]) -> LabelSet {
        LabelSet::from_ids(ids.iter().map(|s| (*s).to_owned()))
    }

    /// SW-04 AC2 / SW-03: trash and spam have their own methods; a set that
    /// carries one, or an immutable system label, is refused.
    #[test]
    fn gmail_set_labels_refuses_trash_and_spam() {
        for forbidden in ["TRASH", "SPAM", "SENT", "DRAFT", "CHAT"] {
            assert!(
                refuse_forbidden(&set(&[forbidden])).is_err(),
                "{forbidden} must be refused"
            );
        }
        assert!(refuse_forbidden(&set(&["INBOX", "UNREAD", "Label_1"])).is_ok());
        assert!(refuse_forbidden(&LabelSet::new()).is_ok());
    }

    /// FL-02 AC1: a user label may not shadow a system label name.
    #[test]
    fn gmail_ensure_label_rejects_system_names() {
        for name in [
            "INBOX",
            "inbox",
            "Spam",
            "TRASH",
            "unread",
            "STARRED",
            "IMPORTANT",
            "SENT",
            "DRAFT",
            "chat",
        ] {
            assert!(
                validate_label_name(name).is_err(),
                "{name:?} must be refused"
            );
        }
        for name in ["Receipts", "Mail Tinder", "a/b", "Team/2026"] {
            assert!(
                validate_label_name(name).is_ok(),
                "{name:?} must be allowed"
            );
        }
        assert!(validate_label_name("").is_err());
        assert!(validate_label_name("/leading").is_err());
        assert!(validate_label_name("trailing/").is_err());
        assert!(validate_label_name("two//slashes").is_err());
        assert!(validate_label_name("bad\u{7f}control").is_err());
        assert!(validate_label_name(&"a".repeat(LABEL_NAME_MAX_CHARS)).is_ok());
        assert!(validate_label_name(&"a".repeat(LABEL_NAME_MAX_CHARS + 1)).is_err());
    }
}
