//! Fail-closed authentication assessment for the unsubscribe headers.
//!
//! T-406 replaces this body with the DKIM coverage check; the signature stays.

use domain::UnsubscribeOptions;

use crate::headers::RawHeaders;

/// What the adapter knows about authentication and unsubscribe headers.
pub struct AuthAssessment {
    /// DKIM-covered unsubscribe options; none until T-406 lands.
    pub unsubscribe: Option<UnsubscribeOptions>,
    /// A `List-Unsubscribe` header exists at all, covered or not.
    pub list_unsubscribe_present: bool,
    /// DKIM-aligned `From` or provider auth pass; false until T-406 lands.
    pub from_authenticated: bool,
}

/// Assess the authentication headers. Fail closed: nothing is covered yet.
pub fn assess_auth(h: &RawHeaders, _from_domain: &str) -> AuthAssessment {
    AuthAssessment {
        unsubscribe: None,
        list_unsubscribe_present: h.count("List-Unsubscribe") > 0,
        from_authenticated: false,
    }
}
