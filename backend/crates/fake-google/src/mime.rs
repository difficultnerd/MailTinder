//! Convert `.eml` bytes into a Gmail `payload` tree (T-205a).
//!
//! Uses `mail-parser` (MIT/Apache-2.0). Builds the recursive `payload` object
//! Gmail returns for `format=full`: `partId`, `mimeType`, `filename`, `headers`
//! (name and decoded value, message order), and `body` with `size` plus `data`
//! (base64url, no padding) for leaf parts, or `attachmentId` for attachments.

use base64::Engine as _;
use mail_parser::{HeaderValue, Message, MessageParser, PartType};
use serde_json::{json, Value};

/// Build the Gmail `payload` for a message's raw bytes.
pub fn payload(raw: &[u8]) -> Value {
    let msg = MessageParser::default().parse(raw).unwrap_or_default();
    part_payload(&msg, 0)
}

/// Build the `payload` for one part (recursively).
fn part_payload(msg: &Message<'_>, part_id: usize) -> Value {
    let part = msg.parts.get(part_id);
    let mut out = json!({
        "partId": part_id.to_string(),
        "mimeType": mime_type(part),
        "headers": headers(part),
    });
    if let Some(filename) = filename(part) {
        out["filename"] = json!(filename);
    }
    match part.map(|p| &p.body) {
        Some(PartType::Multipart(children)) => {
            let parts: Vec<Value> = children.iter().map(|&c| part_payload(msg, c)).collect();
            out["parts"] = json!(parts);
        }
        Some(PartType::Message(nested)) => {
            out["parts"] = json!([part_payload(nested, 0)]);
        }
        Some(PartType::Text(t) | PartType::Html(t)) => {
            out["body"] = json!({
                "size": t.len(),
                "data": base64url_no_pad(t.as_bytes()),
            });
        }
        Some(PartType::Binary(b) | PartType::InlineBinary(b)) => {
            if is_attachment(part) {
                out["body"] = json!({
                    "size": b.len(),
                    "attachmentId": format!("ATTACH-{part_id}"),
                });
            } else {
                out["body"] = json!({
                    "size": b.len(),
                    "data": base64url_no_pad(b),
                });
            }
        }
        None => {
            out["body"] = json!({ "size": 0 });
        }
    }
    out
}

fn mime_type(part: Option<&mail_parser::MessagePart<'_>>) -> String {
    part.and_then(|p| {
        p.headers.iter().find_map(|h| match &h.value {
            HeaderValue::ContentType(ct) => {
                let mut s = ct.c_type.to_string();
                if let Some(sub) = &ct.c_subtype {
                    s.push('/');
                    s.push_str(sub);
                }
                Some(s)
            }
            _ => None,
        })
    })
    .unwrap_or_else(|| "text/plain".to_owned())
}

fn filename(part: Option<&mail_parser::MessagePart<'_>>) -> Option<String> {
    part.and_then(|p| {
        p.headers.iter().find_map(|h| match &h.value {
            HeaderValue::ContentType(ct) => ct.attributes.as_ref().and_then(|attrs| {
                attrs.iter().find_map(|(k, v)| {
                    (k.eq_ignore_ascii_case("name") || k.eq_ignore_ascii_case("filename"))
                        .then(|| v.to_string())
                })
            }),
            _ => None,
        })
    })
}

fn is_attachment(part: Option<&mail_parser::MessagePart<'_>>) -> bool {
    part.is_some_and(|p| {
        p.headers.iter().any(|h| match &h.value {
            HeaderValue::ContentType(ct) => ct.attributes.as_ref().is_some_and(|attrs| {
                attrs
                    .iter()
                    .any(|(k, v)| k.eq_ignore_ascii_case("disposition") && v == "attachment")
            }),
            _ => false,
        })
    })
}

/// The headers as `[{name, value}]` in message order, decoded values.
fn headers(part: Option<&mail_parser::MessagePart<'_>>) -> Vec<Value> {
    let Some(part) = part else {
        return Vec::new();
    };
    part.headers
        .iter()
        .map(|h| {
            json!({
                "name": h.name.to_string(),
                "value": header_value(&h.value),
            })
        })
        .collect()
}

fn header_value(value: &HeaderValue<'_>) -> String {
    match value {
        HeaderValue::Text(t) => t.to_string(),
        HeaderValue::TextList(l) => {
            let mut parts = Vec::with_capacity(l.len());
            for s in l {
                parts.push(s.to_string());
            }
            parts.join(", ")
        }
        HeaderValue::Address(a) => match a {
            mail_parser::Address::List(l) => {
                let mut parts = Vec::with_capacity(l.len());
                for addr in l {
                    let name = addr.name.as_deref().unwrap_or("");
                    let email = addr.address.as_deref().unwrap_or("");
                    parts.push(if name.is_empty() {
                        email.to_owned()
                    } else {
                        format!("{name} <{email}>")
                    });
                }
                parts.join(", ")
            }
            mail_parser::Address::Group(g) => {
                let mut out = Vec::with_capacity(g.len());
                for grp in g {
                    let name = grp.name.as_deref().unwrap_or("");
                    let mut members = Vec::with_capacity(grp.addresses.len());
                    for addr in &grp.addresses {
                        let n = addr.name.as_deref().unwrap_or("");
                        let e = addr.address.as_deref().unwrap_or("");
                        members.push(if n.is_empty() {
                            e.to_owned()
                        } else {
                            format!("{n} <{e}>")
                        });
                    }
                    out.push(format!("{name}: {}", members.join(", ")));
                }
                out.join(", ")
            }
        },
        HeaderValue::DateTime(dt) => format!(
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
            dt.year, dt.month, dt.day, dt.hour, dt.minute, dt.second
        ),
        HeaderValue::ContentType(ct) => {
            let mut s = ct.c_type.to_string();
            if let Some(sub) = &ct.c_subtype {
                s.push('/');
                s.push_str(sub);
            }
            if let Some(attrs) = &ct.attributes {
                for (k, v) in attrs {
                    s.push_str("; ");
                    s.push_str(k);
                    s.push('=');
                    s.push_str(v);
                }
            }
            s
        }
        HeaderValue::Received(r) => {
            let mut s = String::from("from ");
            if let Some(h) = &r.from {
                s.push_str(&h.to_string());
            }
            s.push_str(" by ");
            if let Some(h) = &r.by {
                s.push_str(&h.to_string());
            }
            s
        }
        HeaderValue::Empty => String::new(),
    }
}

fn base64url_no_pad(data: &[u8]) -> String {
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    URL_SAFE_NO_PAD.encode(data)
}
