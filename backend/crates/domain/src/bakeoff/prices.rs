//! Price tables for the bake-off cost figures (S7 5.13, CR-01a G5, STAT-6).
//!
//! Costs are priced per record from the record's own `price_version`, so a run
//! that spans two price tables is still priced correctly rather than refused.

use std::collections::BTreeMap;

use super::report::Method;

/// The two model prices of one price version, in USD per million input tokens.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ModelPrices {
    /// Gemini, per million input tokens.
    pub gemini_usd_per_million_input: f64,
    /// Jev, per million input tokens.
    pub jev_usd_per_million_input: f64,
}

/// The price tables the report prices records with, keyed by `price_version`.
#[derive(Clone, Debug, PartialEq)]
pub struct PriceTable {
    /// One entry per price version the report knows.
    pub versions: BTreeMap<String, ModelPrices>,
}

impl PriceTable {
    /// A table with no versions in it: no record can be priced.
    #[must_use]
    pub fn new() -> Self {
        Self {
            versions: BTreeMap::new(),
        }
    }
}

impl Default for PriceTable {
    fn default() -> Self {
        Self::new()
    }
}

/// The price of one method under one price version, in USD per million input
/// tokens; `None` when the version is unknown.
///
/// Header rules run in process and always cost nothing, whatever the version.
#[must_use]
pub fn price_for(prices: &PriceTable, version: &str, method: Method) -> Option<f64> {
    match method {
        Method::HeaderRules => Some(0.0),
        Method::Gemini => prices
            .versions
            .get(version)
            .map(|prices| prices.gemini_usd_per_million_input),
        Method::Jev => prices
            .versions
            .get(version)
            .map(|prices| prices.jev_usd_per_million_input),
    }
}

/// Price version `"1"` `[TUNABLE]`: Jev 0.042 USD per million input tokens
/// (`research/jev-classifier-evaluation.md`), Gemini 0.10 USD per million input
/// tokens for the pinned Flash-Lite model (Vertex AI price page, Gemini 2.5
/// Flash-Lite, text/image/video input). Header rules always cost 0.
#[must_use]
pub fn default_prices() -> PriceTable {
    let mut versions = BTreeMap::new();
    versions.insert(
        "1".to_owned(),
        ModelPrices {
            gemini_usd_per_million_input: 0.10,
            jev_usd_per_million_input: 0.042,
        },
    );
    PriceTable { versions }
}
