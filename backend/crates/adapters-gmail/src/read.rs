//! The read half of [`MailProvider`] over the Gmail REST API.
//!
//! `list_inbox`, `get_meta`, `get_preview` and `inbox_count` are real; the
//! write methods are stubs filled by T-403 and T-404.

use async_trait::async_trait;
use base64::Engine as _;
use domain::{LabelSet, MailtoTarget, MessageId, MessageMeta, Provider, SenderKey};
use futures::StreamExt as _;
use ports::{
    ListOrder, MailError, MailProvider, MailboxCtx, MessagePage, PageToken, ProviderCapabilities,
};
use serde::Deserialize;
use time::OffsetDateTime;

use crate::client::GmailHttp;
use crate::headers::{
    build_header_facts, decode_header_text, parse_from, ParsedFrom, RawHeaders, METADATA_HEADERS,
};

/// The Feed page size `[TUNABLE]` (S7 2).
pub const LIST_PAGE_SIZE: u32 = 20;
/// Per-message fetches in flight; keeps one page under Gmail's quota rate.
pub const GET_CONCURRENCY: usize = 10;
/// The longest subject kept (S7 Card).
pub const SUBJECT_MAX_CHARS: usize = 998;
/// The longest preview kept `[TUNABLE]` (FD-01 AC2).
pub const PREVIEW_MAX_CHARS: usize = 300;
/// Only the first 256 KiB of a decoded body are used `[DEFAULT]`.
pub const PREVIEW_MAX_BYTES: usize = 256 * 1024;

/// The Gmail implementation of [`MailProvider`].
pub struct GmailProvider {
    http: GmailHttp,
}

impl GmailProvider {
    pub fn new(http: GmailHttp) -> Self {
        Self { http }
    }
}

#[async_trait]
impl MailProvider for GmailProvider {
    fn provider(&self) -> Provider {
        Provider::Gmail
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            labels_are_sets: true,
            spam_is_label: true,
        }
    }

    async fn list_inbox(
        &self,
        mb: &MailboxCtx,
        page: Option<PageToken>,
        order: ListOrder,
    ) -> Result<MessagePage, MailError> {
        let mut query: Vec<(&str, String)> = vec![
            ("labelIds", "INBOX".to_owned()),
            ("maxResults", LIST_PAGE_SIZE.to_string()),
            ("includeSpamTrash", "false".to_owned()),
            ("fields", "messages/id,nextPageToken".to_owned()),
        ];
        if let Some(PageToken(token)) = &page {
            query.push(("pageToken", token.clone()));
        }
        match order {
            ListOrder::NewestFirst => {}
            ListOrder::NewerThan(t) => query.push(("q", format!("after:{}", t.unix_timestamp()))),
            ListOrder::OlderThan(t) => query.push(("q", format!("before:{}", t.unix_timestamp()))),
        }
        let list: ListResponse = self.http.get_json(mb, "messages", &query).await?;

        let fetches = list
            .messages
            .unwrap_or_default()
            .into_iter()
            .map(|only| async move {
                let id = MessageId::new(only.id).map_err(|_| MailError::Transient)?;
                self.get_meta(mb, &id).await
            });
        let results: Vec<Result<MessageMeta, MailError>> = futures::stream::iter(fetches)
            .buffered(GET_CONCURRENCY)
            .collect()
            .await;

        let mut items = Vec::new();
        for result in results {
            match result {
                Ok(meta) => items.push(meta),
                // FD-04 AC1: the message left between list and get; skip it.
                Err(MailError::NotFound) => {}
                Err(other) => return Err(other),
            }
        }
        Ok(MessagePage {
            items,
            next: list.next_page_token.map(PageToken),
        })
    }

    async fn get_meta(&self, mb: &MailboxCtx, id: &MessageId) -> Result<MessageMeta, MailError> {
        let mut query: Vec<(&str, String)> = vec![
            ("format", "metadata".to_owned()),
            (
                "fields",
                "id,labelIds,internalDate,payload/headers".to_owned(),
            ),
        ];
        for name in METADATA_HEADERS {
            query.push(("metadataHeaders", name.to_owned()));
        }
        let path = format!("messages/{}", id.as_str());
        let message: MetaMessage = self.http.get_json(mb, &path, &query).await?;

        let millis: i64 = message
            .internal_date
            .parse()
            .map_err(|_| MailError::Transient)?;
        let internal_date =
            OffsetDateTime::from_unix_timestamp_nanos(i128::from(millis) * 1_000_000)
                .map_err(|_| MailError::Transient)?;

        let headers = RawHeaders(
            message
                .payload
                .headers
                .into_iter()
                .map(|h| (h.name, h.value))
                .collect(),
        );
        let from = headers
            .first("From")
            .and_then(parse_from)
            .unwrap_or_else(empty_from);
        let subject = truncate(
            &decode_header_text(headers.first("Subject").unwrap_or("")),
            SUBJECT_MAX_CHARS,
        );
        let facts = build_header_facts(&headers, &from);
        let labels = LabelSet::from_ids(message.label_ids);
        let sender = SenderKey::from_address(&from.address);

        Ok(MessageMeta {
            mailbox: mb.mailbox,
            id: id.clone(),
            internal_date,
            from_display: from.display,
            from_address: from.address,
            sender,
            subject,
            labels,
            facts,
        })
    }

    async fn get_preview(&self, mb: &MailboxCtx, id: &MessageId) -> Result<String, MailError> {
        let query: Vec<(&str, String)> = vec![
            ("format", "full".to_owned()),
            (
                "fields",
                "payload(mimeType,headers,body/data,body/attachmentId,parts)".to_owned(),
            ),
        ];
        let path = format!("messages/{}", id.as_str());
        let message: FullMessage = self.http.get_json(mb, &path, &query).await?;
        let Some(part) = select_preview_part(&message.payload) else {
            return Ok(String::new());
        };
        let data = part
            .body
            .as_ref()
            .and_then(|b| b.data.as_deref())
            .unwrap_or("");
        let decoded = decode_body(data);
        let used = &decoded[..decoded.len().min(PREVIEW_MAX_BYTES)];
        let text = String::from_utf8_lossy(used);
        // T-402: text/html goes through `domain::text::html_to_text` here.
        Ok(sanitise(&text, PREVIEW_MAX_CHARS))
    }

    async fn inbox_count(&self, mb: &MailboxCtx) -> Result<u64, MailError> {
        let query: Vec<(&str, String)> = vec![("fields", "messagesTotal".to_owned())];
        let label: LabelResponse = self.http.get_json(mb, "labels/INBOX", &query).await?;
        Ok(label.messages_total.unwrap_or(0))
    }

    async fn set_labels(
        &self,
        _mb: &MailboxCtx,
        _id: &MessageId,
        _add: &LabelSet,
        _remove: &LabelSet,
    ) -> Result<LabelSet, MailError> {
        Err(MailError::Invalid("not_implemented".to_owned()))
    }

    async fn trash(&self, _mb: &MailboxCtx, _id: &MessageId) -> Result<LabelSet, MailError> {
        Err(MailError::Invalid("not_implemented".to_owned()))
    }

    async fn report_spam(&self, _mb: &MailboxCtx, _id: &MessageId) -> Result<LabelSet, MailError> {
        Err(MailError::Invalid("not_implemented".to_owned()))
    }

    async fn restore_labels(
        &self,
        _mb: &MailboxCtx,
        _id: &MessageId,
        _exact: &LabelSet,
    ) -> Result<(), MailError> {
        Err(MailError::Invalid("not_implemented".to_owned()))
    }

    async fn ensure_label(&self, _mb: &MailboxCtx, _name: &str) -> Result<String, MailError> {
        Err(MailError::Invalid("not_implemented".to_owned()))
    }

    async fn send_mailto(&self, _mb: &MailboxCtx, _to: &MailtoTarget) -> Result<(), MailError> {
        Err(MailError::Invalid("not_implemented".to_owned()))
    }
}

/// The empty `From` used when the header is absent or unparsable.
fn empty_from() -> ParsedFrom {
    ParsedFrom {
        display: String::new(),
        address: String::new(),
    }
}

/// The first `max` characters of `s`.
fn truncate(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

/// Strip control characters and bound the length.
///
/// T-402 replaces this with `domain::text::sanitise_plain`.
fn sanitise(s: &str, max: usize) -> String {
    s.chars().filter(|c| !c.is_control()).take(max).collect()
}

/// Decode a base64url body (no padding required).
fn decode_body(data: &str) -> Vec<u8> {
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(data.trim_end_matches('=').as_bytes())
        .unwrap_or_default()
}

/// Pick the first `text/plain` part, else the first `text/html` part.
fn select_preview_part(root: &Part) -> Option<&Part> {
    let mut plain = None;
    let mut html = None;
    walk_parts(root, &mut plain, &mut html);
    plain.or(html)
}

fn walk_parts<'a>(part: &'a Part, plain: &mut Option<&'a Part>, html: &mut Option<&'a Part>) {
    if is_attachment(part) {
        return;
    }
    if plain.is_none() && part.mime_type.eq_ignore_ascii_case("text/plain") && part.body.is_some() {
        *plain = Some(part);
    }
    if html.is_none() && part.mime_type.eq_ignore_ascii_case("text/html") && part.body.is_some() {
        *html = Some(part);
    }
    for child in &part.parts {
        walk_parts(child, plain, html);
    }
}

/// A part is skipped when it is an attachment or carries an attachment ID.
fn is_attachment(part: &Part) -> bool {
    if part
        .body
        .as_ref()
        .is_some_and(|b| b.attachment_id.is_some())
    {
        return true;
    }
    part.headers.iter().any(|h| {
        h.name.eq_ignore_ascii_case("Content-Disposition")
            && h.value.to_ascii_lowercase().contains("attachment")
    })
}

// Gmail wire types. Private to this crate; no `deny_unknown_fields` (Gmail adds
// fields over time).

#[derive(Deserialize)]
struct ListResponse {
    messages: Option<Vec<IdOnly>>,
    #[serde(rename = "nextPageToken")]
    next_page_token: Option<String>,
}

#[derive(Deserialize)]
struct IdOnly {
    id: String,
}

#[derive(Deserialize)]
#[allow(dead_code)] // `id` is redundant with the requested ID; kept for the wire shape.
struct MetaMessage {
    id: String,
    #[serde(rename = "labelIds", default)]
    label_ids: Vec<String>,
    #[serde(rename = "internalDate")]
    internal_date: String,
    payload: MetaPayload,
}

#[derive(Deserialize)]
struct MetaPayload {
    #[serde(default)]
    headers: Vec<Header>,
}

#[derive(Deserialize)]
struct Header {
    name: String,
    value: String,
}

#[derive(Deserialize)]
struct FullMessage {
    payload: Part,
}

#[derive(Deserialize)]
struct Part {
    #[serde(rename = "mimeType", default)]
    mime_type: String,
    #[serde(default)]
    headers: Vec<Header>,
    body: Option<PartBody>,
    #[serde(default)]
    parts: Vec<Part>,
}

#[derive(Deserialize)]
struct PartBody {
    data: Option<String>,
    #[serde(rename = "attachmentId")]
    attachment_id: Option<String>,
}

#[derive(Deserialize)]
struct LabelResponse {
    #[serde(rename = "messagesTotal")]
    messages_total: Option<u64>,
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    fn part(mime: &str, data: Option<&str>, attachment_id: Option<&str>) -> Part {
        Part {
            mime_type: mime.to_owned(),
            headers: Vec::new(),
            body: Some(PartBody {
                data: data.map(str::to_owned),
                attachment_id: attachment_id.map(str::to_owned),
            }),
            parts: Vec::new(),
        }
    }

    #[test]
    fn gmail_preview_selects_text_plain_over_html() {
        let mut root = part("multipart/alternative", None, None);
        root.body = None;
        root.parts = vec![
            part("text/plain", Some("plain-wins"), None),
            part("text/html", Some("<p>html</p>"), None),
        ];
        let picked = select_preview_part(&root).expect("a part");
        assert_eq!(picked.mime_type, "text/plain");
    }

    #[test]
    fn gmail_preview_skips_attachments() {
        let mut root = part("multipart/mixed", None, None);
        root.body = None;
        root.parts = vec![
            // base64url of "attached".
            part("text/plain", Some("YXR0YWNoZWQ"), Some("ATTACH-1")),
            // base64url of "hello".
            part("text/plain", Some("aGVsbG8"), None),
        ];
        let picked = select_preview_part(&root).expect("a part");
        let decoded = decode_body(
            picked
                .body
                .as_ref()
                .and_then(|b| b.data.as_deref())
                .unwrap(),
        );
        assert_eq!(String::from_utf8_lossy(&decoded), "hello");
    }

    #[test]
    fn gmail_preview_decodes_base64url_without_padding() {
        // "hello" -> aGVsbG8 (no padding).
        assert_eq!(decode_body("aGVsbG8"), b"hello");
    }

    #[test]
    fn gmail_preview_sanitise_strips_control_and_bounds_length() {
        assert_eq!(sanitise("a\u{0}b\nc", 300), "abc");
        assert_eq!(sanitise("abcdef", 3), "abc");
    }
}
