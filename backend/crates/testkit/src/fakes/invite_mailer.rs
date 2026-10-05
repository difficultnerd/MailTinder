//! A recording `InviteMailer` fake (T-404).
//!
//! It proves the invite half without a network: each `send_invite` records the
//! recipient and the link string, so a test can assert the admin's mailbox sent
//! exactly the fixed message to exactly the invited address.

use std::sync::Mutex;

use async_trait::async_trait;
use domain::EmailAddress;
use ports::{InviteLink, InviteMailer, MailError, MailboxCtx};

/// Records each `(to, link)` pair it is asked to send.
#[derive(Default)]
pub struct FakeInviteMailer {
    sent: Mutex<Vec<(EmailAddress, String)>>,
    fail_next: Mutex<bool>,
}

impl FakeInviteMailer {
    pub fn new() -> Self {
        Self::default()
    }

    /// The `(to, link)` pairs sent so far, in order.
    pub fn sent(&self) -> Vec<(EmailAddress, String)> {
        self.sent
            .lock()
            .unwrap_or_else(|_| panic!("invite mailer poisoned"))
            .clone()
    }

    /// Make the next `send_invite` fail with [`MailError::Transient`].
    pub fn fail_next(&self) {
        *self
            .fail_next
            .lock()
            .unwrap_or_else(|_| panic!("invite mailer poisoned")) = true;
    }
}

#[async_trait]
impl InviteMailer for FakeInviteMailer {
    async fn send_invite(
        &self,
        _mb: &MailboxCtx,
        to: &EmailAddress,
        link: &InviteLink,
    ) -> Result<(), MailError> {
        {
            let mut fail = self
                .fail_next
                .lock()
                .unwrap_or_else(|_| panic!("invite mailer poisoned"));
            if *fail {
                *fail = false;
                return Err(MailError::Transient);
            }
        }
        self.sent
            .lock()
            .unwrap_or_else(|_| panic!("invite mailer poisoned"))
            .push((to.clone(), link.0.expose().as_str().to_owned()));
        Ok(())
    }
}
