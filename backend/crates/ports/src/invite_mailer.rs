//! The narrow invite-email port (AU-01 AC1).
//!
//! `InviteMailer` can send exactly one message: the fixed invite email carrying
//! the sign-in link. There is no subject, body or recipient free enough to
//! abuse; the subject and body are constants in the adapter and the link is
//! checked before the send.

use async_trait::async_trait;
use domain::EmailAddress;
use obs::Sensitive;
use url::Url;

use crate::mail::{MailError, MailboxCtx};

/// `https://<app origin>/#/invite?t=<token>`; built only by the api (T-505).
/// Debug prints `[redacted]` through the wrapped [`Sensitive`].
pub struct InviteLink(pub Sensitive<Url>);

/// Sends the fixed invite email.
#[async_trait]
pub trait InviteMailer: Send + Sync {
    /// Sends the fixed invite email from `mb` to `to`. No other content is
    /// possible.
    async fn send_invite(
        &self,
        mb: &MailboxCtx,
        to: &EmailAddress,
        link: &InviteLink,
    ) -> Result<(), MailError>;
}
