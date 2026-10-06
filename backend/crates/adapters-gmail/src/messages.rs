//! Typed message queries over Gmail (T-602a): `list_messages`,
//! `count_messages` and `web_url`.
//!
//! The typed [`MessageQuery`] is turned into Gmail's `q` and `labelIds` here and
//! nowhere else. Every free-text value is validated against a closed alphabet
//! before it is placed in `q`; anything else is refused without a provider
//! call, so a caller can never smuggle a Gmail search operator through.

use domain::{MessageId, MessageMeta};
use futures::StreamExt as _;
use ports::{MailError, MailboxCtx, MessagePage, MessageQuery, PageToken};
use serde::Deserialize;
use url::Url;

use crate::read::{GmailProvider, GET_CONCURRENCY};

/// The longest `from` or `list_id` value accepted (an address maximum).
const QUERY_VALUE_MAX_BYTES: usize = 320;
/// The longest label ID accepted; Gmail IDs are far shorter.
const LABEL_ID_MAX_BYTES: usize = 64;
/// The web client entry point.
const WEB_BASE: &str = "https://mail.google.com/mail/";

#[derive(Deserialize)]
struct ListResponse {
    messages: Option<Vec<IdOnly>>,
    #[serde(rename = "nextPageToken")]
    next_page_token: Option<String>,
    #[serde(rename = "resultSizeEstimate")]
    result_size_estimate: Option<u64>,
}

#[derive(Deserialize)]
struct IdOnly {
    id: String,
}

#[derive(Deserialize)]
struct LabelTotal {
    #[serde(rename = "messagesTotal")]
    messages_total: Option<u64>,
}

/// The invalid-query error: a fixed reason, never the offending text (XC-01).
fn invalid_query() -> MailError {
    MailError::Invalid("query".to_owned())
}

/// True for a byte allowed in a `from` or `list_id` value.
fn query_byte_ok(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'@' | b'.' | b'_' | b'+' | b'-')
}

/// `from` / `list_id`: 1 to 320 bytes drawn from `A-Z a-z 0-9 @ . _ + -`.
fn validate_value(value: &str) -> Result<&str, MailError> {
    if value.is_empty() || value.len() > QUERY_VALUE_MAX_BYTES || !value.bytes().all(query_byte_ok)
    {
        return Err(invalid_query());
    }
    Ok(value)
}

/// A label ID goes into a URL path, so it is held to `A-Za-z0-9_-`.
pub(crate) fn validate_label_id(label_id: &str) -> Result<&str, MailError> {
    let ok = !label_id.is_empty()
        && label_id.len() <= LABEL_ID_MAX_BYTES
        && label_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'));
    if ok {
        Ok(label_id)
    } else {
        Err(invalid_query())
    }
}

/// The `labelIds` values and `q` string for a query, validated first.
fn build_filters(q: &MessageQuery) -> Result<(Vec<String>, Option<String>), MailError> {
    let from = q.from.as_deref().map(validate_value).transpose()?;
    let list_id = q.list_id.as_deref().map(validate_value).transpose()?;
    let label = q.label.as_deref().map(validate_label_id).transpose()?;

    let mut labels = Vec::new();
    if q.in_inbox {
        labels.push("INBOX".to_owned());
    }
    if let Some(label) = label {
        labels.push(label.to_owned());
    }

    let mut terms = Vec::new();
    if let Some(after) = q.after {
        terms.push(format!("after:{}", after.unix_timestamp()));
    }
    if let Some(before) = q.before {
        terms.push(format!("before:{}", before.unix_timestamp()));
    }
    if let Some(from) = from {
        terms.push(format!("from:{from}"));
    }
    if let Some(list_id) = list_id {
        terms.push(format!("list:{list_id}"));
    }
    let text = if terms.is_empty() {
        None
    } else {
        Some(terms.join(" "))
    };
    Ok((labels, text))
}

/// The single label a query counts exactly, when it is "label only" or "inbox
/// only"; `None` for any other shape.
fn exact_count_label(q: &MessageQuery) -> Result<Option<String>, MailError> {
    let (labels, text) = build_filters(q)?;
    if text.is_some() || labels.len() != 1 {
        return Ok(None);
    }
    Ok(labels.into_iter().next())
}

/// The `messages.list` query pairs for `q`.
fn list_params(
    q: &MessageQuery,
    page: Option<&PageToken>,
    max: u32,
    fields: &'static str,
) -> Result<Vec<(&'static str, String)>, MailError> {
    let (labels, text) = build_filters(q)?;
    let mut params: Vec<(&'static str, String)> = Vec::new();
    for label in labels {
        params.push(("labelIds", label));
    }
    params.push(("maxResults", max.clamp(1, 100).to_string()));
    params.push(("includeSpamTrash", "false".to_owned()));
    params.push(("fields", fields.to_owned()));
    if let Some(PageToken(token)) = page {
        params.push(("pageToken", token.clone()));
    }
    if let Some(text) = text {
        params.push(("q", text));
    }
    Ok(params)
}

/// The provider web URL for a message, built with the URL encoder.
pub(crate) fn web_url_for(mailbox_address: &str, id: &MessageId) -> String {
    let Ok(mut url) = Url::parse(WEB_BASE) else {
        return WEB_BASE.to_owned();
    };
    url.query_pairs_mut()
        .append_pair("authuser", mailbox_address);
    let encoded: String = url::form_urlencoded::byte_serialize(id.as_str().as_bytes()).collect();
    url.set_fragment(Some(&format!("all/{encoded}")));
    url.into()
}

impl GmailProvider {
    /// `messages.list` with a typed query, then `get_meta` per ID.
    pub(crate) async fn list_messages_query(
        &self,
        mb: &MailboxCtx,
        q: &MessageQuery,
        page: Option<PageToken>,
        max: u32,
    ) -> Result<MessagePage, MailError> {
        let params = list_params(
            q,
            page.as_ref(),
            max,
            "messages/id,nextPageToken,resultSizeEstimate",
        )?;
        let list: ListResponse = self.http().get_json(mb, "messages", &params).await?;

        let fetches = list
            .messages
            .unwrap_or_default()
            .into_iter()
            .map(|only| async move {
                let id = MessageId::new(only.id).map_err(|_| MailError::Transient)?;
                ports::MailProvider::get_meta(self, mb, &id).await
            });
        let results: Vec<Result<MessageMeta, MailError>> = futures::stream::iter(fetches)
            .buffered(GET_CONCURRENCY)
            .collect()
            .await;

        let mut items = Vec::new();
        for result in results {
            match result {
                Ok(meta) => items.push(meta),
                // The message left between list and get; skip it (FD-04 AC1).
                Err(MailError::NotFound) => {}
                Err(other) => return Err(other),
            }
        }
        Ok(MessagePage {
            items,
            next: list.next_page_token.map(PageToken),
        })
    }

    /// Exact `messagesTotal` for a label-only or inbox-only query; otherwise one
    /// `messages.list` call and its `resultSizeEstimate` (GM-04 AC1).
    pub(crate) async fn count_messages_query(
        &self,
        mb: &MailboxCtx,
        q: &MessageQuery,
    ) -> Result<u64, MailError> {
        if let Some(label) = exact_count_label(q)? {
            let query = vec![("fields", "messagesTotal".to_owned())];
            let path = format!("labels/{label}");
            let total: LabelTotal = self.http().get_json(mb, &path, &query).await?;
            return Ok(total.messages_total.unwrap_or(0));
        }
        let params = list_params(q, None, 1, "resultSizeEstimate")?;
        let list: ListResponse = self.http().get_json(mb, "messages", &params).await?;
        Ok(list.result_size_estimate.unwrap_or(0))
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use time::OffsetDateTime;

    fn from(value: &str) -> MessageQuery {
        MessageQuery {
            from: Some(value.to_owned()),
            ..MessageQuery::default()
        }
    }

    #[test]
    fn xc_02_query_with_quote_refused() {
        // The pure builder runs before any request is made, so an `Invalid`
        // here means no HTTP call can follow.
        for bad in ["a\"b@example.com", "a b@example.com", "x:y", "", "a(b)"] {
            assert_eq!(
                build_filters(&from(bad)).unwrap_err(),
                MailError::Invalid("query".to_owned()),
                "{bad:?}"
            );
        }
        let long = "a".repeat(321);
        assert!(build_filters(&from(&long)).is_err());
        let list = MessageQuery {
            list_id: Some("l\"x".to_owned()),
            ..MessageQuery::default()
        };
        assert!(list_params(&list, None, 10, "x").is_err());
    }

    #[test]
    fn xc_02_query_joins_terms_with_single_spaces() {
        let q = MessageQuery {
            in_inbox: true,
            label: Some("Label_7".to_owned()),
            after: Some(OffsetDateTime::from_unix_timestamp(100).unwrap()),
            before: Some(OffsetDateTime::from_unix_timestamp(200).unwrap()),
            from: Some("a+b@example.com".to_owned()),
            list_id: Some("news.example.com".to_owned()),
        };
        let (labels, text) = build_filters(&q).unwrap();
        assert_eq!(labels, vec!["INBOX", "Label_7"]);
        assert_eq!(
            text.as_deref(),
            Some("after:100 before:200 from:a+b@example.com list:news.example.com")
        );
    }

    #[test]
    fn xc_02_exact_count_only_for_single_label_queries() {
        let inbox = MessageQuery {
            in_inbox: true,
            ..MessageQuery::default()
        };
        assert_eq!(exact_count_label(&inbox).unwrap().as_deref(), Some("INBOX"));
        let dated = MessageQuery {
            in_inbox: true,
            after: Some(OffsetDateTime::from_unix_timestamp(1).unwrap()),
            ..MessageQuery::default()
        };
        assert_eq!(exact_count_label(&dated).unwrap(), None);
        assert_eq!(exact_count_label(&MessageQuery::default()).unwrap(), None);
    }

    #[test]
    fn xc_02_web_url_encodes_address_and_id() {
        let id = MessageId::new("abc123").unwrap();
        assert_eq!(
            web_url_for("me+x@example.com", &id),
            "https://mail.google.com/mail/?authuser=me%2Bx%40example.com#all/abc123"
        );
    }
}
