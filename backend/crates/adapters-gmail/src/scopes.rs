//! The exact OAuth scopes every flow may request (S6 section 4; AU-04 AC5).
//!
//! The lists are closed: a flow that needs a new scope must change this file,
//! so a reviewer sees every scope the app can ever ask Google for. Nothing
//! else is ever requested.

/// The scopes requested for a Gmail mailbox (S6 section 4).
pub const GMAIL_SCOPES: [&str; 5] = [
    "openid",
    "email",
    "https://www.googleapis.com/auth/gmail.modify",
    "https://www.googleapis.com/auth/gmail.send",
    "https://www.googleapis.com/auth/drive.appdata",
];

/// The scopes requested for a step-up sign-in: no new scopes (S7 API-AUTH-1).
pub const STEP_UP_SCOPES: [&str; 2] = ["openid", "email"];
