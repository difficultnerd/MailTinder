//! The OAuth callback outcome table (S7 3.4) and the one redirect it produces.

use axum::http::{header, HeaderValue, StatusCode};
use axum::response::Response;

use crate::config::ApiConfig;
use crate::session::store::NewCookie;

/// Every outcome the callback can report. Matched without `_` so a new value
/// forces every site to be reviewed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// A linked mailbox signed the user in.
    SignedIn,
    /// An invite token was redeemed and the user created.
    Joined,
    /// A mailbox was linked to the current user (T-601a).
    Linked,
    /// A mailbox in `needs_sign_in` was refreshed (T-601a).
    Reconnected,
    /// A fresh sign-in for step-up (T-504).
    SteppedUp,
    /// The person has no account and no invite token: request an invite.
    NotInvited,
    /// The Google account is linked to no user.
    NotRegistered,
    /// The invite is missing, used, expired, revoked or replaced.
    InviteInvalid,
    /// The Google email is not the invited address.
    EmailMismatch,
    /// Google says the email is not verified.
    EmailUnverified,
    /// The mailbox is already linked to another user.
    MailboxLinkedElsewhere,
    /// The step-up account is not one of the user's mailboxes (T-504).
    StepUpWrongAccount,
    /// Microsoft work tenant blocked consent (v2).
    ConsentBlocked,
    /// The user cancelled at Google.
    Cancelled,
    /// Anything else went wrong.
    Failed,
}

impl Outcome {
    /// The wire code in the redirect URL.
    #[must_use]
    pub fn code(self) -> &'static str {
        match self {
            Self::SignedIn => "signed_in",
            Self::Joined => "joined",
            Self::Linked => "linked",
            Self::Reconnected => "reconnected",
            Self::SteppedUp => "stepped_up",
            Self::NotInvited => "not_invited",
            Self::NotRegistered => "not_registered",
            Self::InviteInvalid => "invite_invalid",
            Self::EmailMismatch => "email_mismatch",
            Self::EmailUnverified => "email_unverified",
            Self::MailboxLinkedElsewhere => "mailbox_linked_elsewhere",
            Self::StepUpWrongAccount => "step_up_wrong_account",
            Self::ConsentBlocked => "consent_blocked",
            Self::Cancelled => "cancelled",
            Self::Failed => "failed",
        }
    }

    /// True only for the outcomes whose provider tokens are kept (S7 3.4).
    #[must_use]
    pub fn keeps_tokens(self) -> bool {
        matches!(
            self,
            Self::SignedIn | Self::Joined | Self::Linked | Self::Reconnected
        )
    }
}

/// A `302` to `{app_origin}/#/auth/result?outcome=<code>`, plus an optional
/// `Set-Cookie`. Nothing else ever reaches the URL, so no personal data does
/// (V10.1.1, V14.2.1, V3.7.2).
#[must_use]
pub fn redirect(config: &ApiConfig, outcome: Outcome, cookie: Option<NewCookie>) -> Response {
    let location = format!(
        "{}/#/auth/result?outcome={}",
        config.app_origin,
        outcome.code()
    );
    let mut resp = Response::new(axum::body::Body::empty());
    *resp.status_mut() = StatusCode::FOUND;
    if let Ok(value) = HeaderValue::from_str(&location) {
        resp.headers_mut().insert(header::LOCATION, value);
    }
    if let Some(cookie) = cookie {
        resp.headers_mut().insert(header::SET_COOKIE, cookie.0);
    }
    resp
}
