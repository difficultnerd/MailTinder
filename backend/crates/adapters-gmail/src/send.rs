//! The send half of [`MailProvider`] and the invite mailer over the Gmail REST
//! API (T-404).
//!
//! Base, headers, timeout and error mapping are the ones in [`crate::client`]
//! (T-401). A send is never retried: a timeout after Gmail accepted the message
//! would send twice. The invite mailer can send only its fixed message.

use async_trait::async_trait;
use base64::Engine as _;
use domain::{EmailAddress, LabelSet, MailtoTarget, MessageId, MAILTO_DEFAULT_TEXT};
use obs::{op_log, OpLog};
use ports::{InviteLink, InviteMailer, MailError, MailboxCtx};
use serde::{Deserialize, Serialize};

use crate::modify::MAIL_TINDER_LABEL;
use crate::read::GmailProvider;

/// The fixed invite subject (AU-01 AC1).
pub const INVITE_SUBJECT: &str = "You're invited to Mail Tinder";
/// The fixed invite body; `{link}` is the only substitution.
pub const INVITE_BODY_TEMPLATE: &str = "You've been invited to try Mail Tinder.\r\n\r\nOpen this link and continue with this Google account:\r\n{link}\r\n\r\nThe link works once and expires in 7 days.\r\n";
/// The base64 body line length (RFC 2045).
const BODY_LINE_CHARS: usize = 76;
/// The most UTF-8 bytes one RFC 2047 encoded word may carry, keeping
/// `=?UTF-8?B?` + base64 + `?=` inside the 75-character limit.
const ENCODED_WORD_MAX_BYTES: usize = 45;

#[derive(Serialize)]
struct SendBody<'a> {
    raw: &'a str,
}

#[derive(Deserialize)]
struct SendResponse {
    id: String,
}

#[derive(Deserialize)]
struct ProfileResponse {
    #[serde(rename = "emailAddress")]
    email_address: String,
}

impl GmailProvider {
    /// Send `target`'s exact message from `mb`, then best-effort label it.
    ///
    /// A labelling failure is logged and swallowed: the message is already sent,
    /// so reporting an error would make the caller send it again.
    pub(crate) async fn send_mailto_target(
        &self,
        mb: &MailboxCtx,
        target: &MailtoTarget,
    ) -> Result<(), MailError> {
        let from = self.profile_address(mb).await?;
        let to = EmailAddress::parse(target.to())
            .map_err(|_| MailError::Invalid("mailto_target".to_owned()))?;
        let subject = target.subject().unwrap_or(MAILTO_DEFAULT_TEXT);
        let body = target.body().unwrap_or(MAILTO_DEFAULT_TEXT);
        let raw = build_rfc5322(&from, &to, subject, body);
        let sent = self.send_raw(mb, raw).await?;
        let id = MessageId::new(sent).map_err(|_| MailError::Transient)?;
        if self.label_sent(mb, &id).await.is_err() {
            log_label_failed();
        }
        Ok(())
    }

    /// The mailbox's own address, from `users.getProfile`.
    async fn profile_address(&self, mb: &MailboxCtx) -> Result<EmailAddress, MailError> {
        let query = vec![("fields", "emailAddress".to_owned())];
        let profile: ProfileResponse = self.http().get_json(mb, "profile", &query).await?;
        EmailAddress::parse(&profile.email_address).map_err(|_| MailError::Transient)
    }

    /// `POST /messages/send`; returns the new message ID.
    async fn send_raw(&self, mb: &MailboxCtx, raw: Vec<u8>) -> Result<String, MailError> {
        let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&raw);
        let query = vec![("fields", "id".to_owned())];
        let body = SendBody { raw: &encoded };
        let sent: SendResponse = self
            .http()
            .post_json(mb, "messages/send", &query, &body)
            .await?;
        Ok(sent.id)
    }

    /// File the sent message under the "Mail Tinder" label (UN-03 AC2).
    async fn label_sent(&self, mb: &MailboxCtx, id: &MessageId) -> Result<(), MailError> {
        let label = self.ensure_label_named(mb, MAIL_TINDER_LABEL).await?;
        let add = LabelSet::from_ids([label]);
        self.modify_labels(mb, id, &add, &LabelSet::new()).await?;
        Ok(())
    }
}

#[async_trait]
impl InviteMailer for GmailProvider {
    async fn send_invite(
        &self,
        mb: &MailboxCtx,
        to: &EmailAddress,
        link: &InviteLink,
    ) -> Result<(), MailError> {
        if !valid_invite_link(link) {
            return Err(MailError::Invalid("invite_link".to_owned()));
        }
        let from = self.profile_address(mb).await?;
        let body = INVITE_BODY_TEMPLATE.replace("{link}", link.0.expose().as_str());
        let raw = build_rfc5322(&from, to, INVITE_SUBJECT, &body);
        let _sent = self.send_raw(mb, raw).await?;
        Ok(())
    }
}

/// True when `link` is `https` with an `/invite?t=` fragment (V1.2.2).
fn valid_invite_link(link: &InviteLink) -> bool {
    let url = link.0.expose();
    url.scheme() == "https" && url.fragment().is_some_and(|f| f.starts_with("/invite?t="))
}

/// Log a best-effort labelling failure: route and outcome only.
fn log_label_failed() {
    op_log(&OpLog {
        op: "gmail.messages.send",
        outcome: "sent_label_failed",
        status: None,
        latency_ms: None,
    });
}

/// Build the RFC 5322 message bytes. Only these headers are ever written.
fn build_rfc5322(from: &EmailAddress, to: &EmailAddress, subject: &str, body: &str) -> Vec<u8> {
    let mut message = String::new();
    for (name, value) in [("From", from.as_str()), ("To", to.as_str())] {
        message.push_str(name);
        message.push_str(": ");
        message.push_str(value);
        message.push_str("\r\n");
    }
    message.push_str("Subject: ");
    message.push_str(&header_subject(subject));
    message.push_str("\r\n");
    message.push_str("MIME-Version: 1.0\r\n");
    message.push_str("Content-Type: text/plain; charset=utf-8\r\n");
    message.push_str("Content-Transfer-Encoding: base64\r\n");
    message.push_str("\r\n");
    let encoded = base64::engine::general_purpose::STANDARD.encode(body.as_bytes());
    for line in encoded.as_bytes().chunks(BODY_LINE_CHARS) {
        for &byte in line {
            message.push(char::from(byte));
        }
        message.push_str("\r\n");
    }
    message.into_bytes()
}

/// A printable-ASCII subject as is, else RFC 2047 encoded words.
fn header_subject(subject: &str) -> String {
    if subject.chars().all(|c| c.is_ascii() && !c.is_control()) {
        subject.to_owned()
    } else {
        encoded_words(subject)
    }
}

/// Split `text` into encoded words of at most 75 characters, folded with CRLF
/// and a single space. The split honours UTF-8 character boundaries.
fn encoded_words(text: &str) -> String {
    let mut words = Vec::new();
    let mut chunk = String::new();
    for ch in text.chars() {
        if !chunk.is_empty() && chunk.len() + ch.len_utf8() > ENCODED_WORD_MAX_BYTES {
            words.push(encoded_word(&chunk));
            chunk.clear();
        }
        chunk.push(ch);
    }
    if !chunk.is_empty() {
        words.push(encoded_word(&chunk));
    }
    words.join("\r\n ")
}

/// One RFC 2047 encoded word.
fn encoded_word(chunk: &str) -> String {
    format!(
        "=?UTF-8?B?{}?=",
        base64::engine::general_purpose::STANDARD.encode(chunk.as_bytes())
    )
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use url::Url;

    fn address(raw: &str) -> EmailAddress {
        EmailAddress::parse(raw).expect("valid address")
    }

    fn links(raw: &str) -> InviteLink {
        InviteLink(obs::Sensitive::new(Url::parse(raw).expect("valid url")))
    }

    /// ASVS V1.2.2: only an `https` URL with an `/invite?t=` fragment passes.
    #[test]
    fn asvs_v1_2_2_invite_link_must_be_https_invite_fragment() {
        assert!(valid_invite_link(&links(
            "https://app.example.com/#/invite?t=abc"
        )));
        for bad in [
            "http://app.example.com/#/invite?t=abc",
            "https://app.example.com/#/join?t=abc",
            "https://app.example.com/",
            "javascript:alert(1)",
        ] {
            assert!(!valid_invite_link(&links(bad)), "{bad} must be refused");
        }
    }

    /// A non-ASCII subject is written as RFC 2047 encoded words, folded and
    /// decodable back to the original.
    #[test]
    fn gmail_subject_non_ascii_encoded_word() {
        let subject = "Über café — résumé".repeat(6);
        let header = header_subject(&subject);
        assert!(header.starts_with("=?UTF-8?B?"));
        for line in header.split("\r\n ") {
            assert!(line.len() <= 75, "encoded word too long: {line}");
        }

        let decoded: String = header
            .split("\r\n ")
            .map(|word| {
                let b64 = word
                    .strip_prefix("=?UTF-8?B?")
                    .and_then(|rest| rest.strip_suffix("?="))
                    .expect("encoded word shape");
                String::from_utf8(
                    base64::engine::general_purpose::STANDARD
                        .decode(b64)
                        .expect("base64"),
                )
                .expect("utf-8")
            })
            .collect();
        assert_eq!(decoded, subject);
    }

    /// A header block is built with the base64 body wrapped at 76 characters.
    #[test]
    fn gmail_build_rfc5322_headers_and_base64_body() {
        let raw = build_rfc5322(
            &address("Me@Example.com"),
            &address("you@example.org"),
            "Hi",
            "hello world",
        );
        let text = String::from_utf8(raw).expect("ascii message");
        assert!(text.contains("From: Me@example.com\r\n"));
        assert!(text.contains("To: you@example.org\r\n"));
        assert!(text.contains("Subject: Hi\r\n"));
        assert!(text.contains("Content-Transfer-Encoding: base64\r\n\r\n"));
        let body = text.split("\r\n\r\n").nth(1).expect("body");
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(body.replace("\r\n", ""))
            .expect("base64");
        assert_eq!(String::from_utf8(decoded).expect("utf-8"), "hello world");
    }
}
