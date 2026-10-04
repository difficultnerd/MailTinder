//! Sender key normalisation and per-sender statistics.

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::ids::CategoryId;

/// Domains treated as forwarding relays whose addresses are unwrapped to the
/// original sender (`SR-01 AC1a`). Extendable.
pub const RELAY_DOMAINS: [&str; 1] = ["icloud.com"];

/// A normalised sender key. Personal data; Debug redacted, no Display.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SenderKey(String);

impl SenderKey {
    /// Trim whitespace and one pair of surrounding angle brackets, lower-case,
    /// and unwrap a known forwarding relay (`SR-01 AC1a`).
    pub fn from_address(address: &str) -> SenderKey {
        let mut s = address.trim().to_lowercase();
        if s.len() >= 2 && s.starts_with('<') && s.ends_with('>') {
            s = s[1..s.len() - 1].to_string();
        }
        if let Some(unwrapped) = unwrap_relay(&s) {
            s = unwrapped;
        }
        SenderKey(s)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The part after the last `@`, or `""` when there is none.
    pub fn domain(&self) -> &str {
        match self.0.rsplit_once('@') {
            Some((_, domain)) => domain,
            None => "",
        }
    }

    /// True when the local part, with `-`, `_` and `.` removed, is `noreply`,
    /// `donotreply` or starts with `noreply`.
    pub fn is_noreply(&self) -> bool {
        let local = self.0.split('@').next().unwrap_or_default();
        let cleaned: String = local
            .chars()
            .filter(|c| !matches!(c, '-' | '_' | '.'))
            .collect();
        cleaned == "noreply" || cleaned == "donotreply" || cleaned.starts_with("noreply")
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Debug for SenderKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SenderKey([redacted])")
    }
}

/// Unwrap a known relay address. Returns `Some(unwrapped)` when the address is
/// a relay address that passes every check, else `None` (keep as is).
fn unwrap_relay(address: &str) -> Option<String> {
    let (local, domain) = address.rsplit_once('@')?;
    if !RELAY_DOMAINS.contains(&domain) {
        return None;
    }
    let (orig_local, rest) = local.rsplit_once("_at_")?;
    // rest must be `<domain with dots as underscores>_<suffix>`.
    let (underscored_domain, suffix) = rest.rsplit_once('_')?;
    if suffix.len() < 4 || !suffix.chars().all(|c| c.is_ascii_alphanumeric()) {
        return None;
    }
    let candidate: String = underscored_domain.replace('_', ".");
    if !candidate.contains('.')
        || !candidate
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '.' || c == '-')
    {
        return None;
    }
    Some(format!("{orig_local}@{candidate}"))
}

/// Per-sender statistics, stored in the user state file (`T-602b`). Fields in
/// this order. Never log a `SenderStats` (C2).
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct SenderStats {
    /// Last seen display name, plain text.
    pub display: String,
    /// Cards shown (GM-08).
    pub seen: u32,
    pub keeps: u32,
    /// Authenticated rejects only, oldest first, keep the last 20.
    pub rejects_counted: Vec<OffsetDateTime>,
    /// `CategoryId` -> times filed.
    pub files: BTreeMap<Uuid, u32>,
    pub last_filed: Option<CategoryId>,
    pub last_seen: Option<OffsetDateTime>,
    pub block_prompt_declined_until: Option<OffsetDateTime>,
    pub boss_defeated: bool,
}

/// How many authenticated rejects are kept in `SenderStats::rejects_counted`.
pub const REJECTS_KEPT: usize = 20;
