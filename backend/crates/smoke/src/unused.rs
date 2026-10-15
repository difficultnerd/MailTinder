//! The ports the smoke checks never touch, as one inert adapter.
//!
//! `Ports` is the whole I/O boundary of a service, so building one needs a
//! value for every field - but `mt-smoke` holds roles for KMS, Firestore and
//! Cloud Tasks only (`infra/terraform/envs/staging/smoke.tf`, S11: "staging
//! smoke"). It has no Secret Manager, Gmail or Drive access, and none of the
//! checks below reaches mail, Drive, the identity provider, egress, secrets or
//! the internal-caller verifier. Rather than grant it roles it must not have,
//! every such port is this one type: each method returns a refusal, so a check
//! that ever reached one fails loudly instead of silently passing.
//!
//! This deliberately does not pull `testkit` into a binary (S10 3.1: testkit is
//! dev-only); the trait set is small and every method is one line.

use async_trait::async_trait;
use domain::{EmailAddress, LabelSet, MailtoTarget, MessageId, MessageMeta, Provider};
use obs::Sensitive;
use ports::{
    AppFolderError, AppFolderStore, AuthRequest, CallerAuthError, CallerVerifier, ETag,
    EgressError, EgressRequest, EgressResponse, HttpEgress, IdClaims, IdError, IdentityProvider,
    InviteLink, InviteMailer, KeyError, ListOrder, MailError, MailProvider, MailboxCtx,
    MessagePage, MessageQuery, OneClickOutcome, PageToken, ProviderCapabilities, SecretError,
    SecretName, Secrets, SystemAad, SystemKeyService, TokenSet, VerifiedCaller,
};
use url::Url;

/// The static origin `authorize_url` reports. Not a real host, never fetched;
/// the smoke run never starts an OAuth round trip.
const UNUSED_ORIGIN: &str = "https://smoke.invalid/";

/// Every port the smoke checks do not exercise. One type, so the composition
/// root stays one expression per field.
pub struct Unused;

/// A refusal, used by every method that returns a `MailError`.
fn mail_refused() -> MailError {
    MailError::Forbidden
}

#[async_trait]
impl MailProvider for Unused {
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
        _mb: &MailboxCtx,
        _page: Option<PageToken>,
        _order: ListOrder,
    ) -> Result<MessagePage, MailError> {
        Err(mail_refused())
    }

    async fn get_meta(&self, _mb: &MailboxCtx, _id: &MessageId) -> Result<MessageMeta, MailError> {
        Err(mail_refused())
    }

    async fn get_preview(&self, _mb: &MailboxCtx, _id: &MessageId) -> Result<String, MailError> {
        Err(mail_refused())
    }

    async fn get_text(
        &self,
        _mb: &MailboxCtx,
        _id: &MessageId,
        _max_chars: usize,
    ) -> Result<String, MailError> {
        Err(mail_refused())
    }

    async fn set_labels(
        &self,
        _mb: &MailboxCtx,
        _id: &MessageId,
        _add: &LabelSet,
        _remove: &LabelSet,
    ) -> Result<LabelSet, MailError> {
        Err(mail_refused())
    }

    async fn trash(&self, _mb: &MailboxCtx, _id: &MessageId) -> Result<LabelSet, MailError> {
        Err(mail_refused())
    }

    async fn report_spam(&self, _mb: &MailboxCtx, _id: &MessageId) -> Result<LabelSet, MailError> {
        Err(mail_refused())
    }

    async fn restore_labels(
        &self,
        _mb: &MailboxCtx,
        _id: &MessageId,
        _exact: &LabelSet,
    ) -> Result<(), MailError> {
        Err(mail_refused())
    }

    async fn ensure_label(&self, _mb: &MailboxCtx, _name: &str) -> Result<String, MailError> {
        Err(mail_refused())
    }

    async fn send_mailto(&self, _mb: &MailboxCtx, _to: &MailtoTarget) -> Result<(), MailError> {
        Err(mail_refused())
    }

    async fn inbox_count(&self, _mb: &MailboxCtx) -> Result<u64, MailError> {
        Err(mail_refused())
    }

    async fn list_messages(
        &self,
        _mb: &MailboxCtx,
        _q: &MessageQuery,
        _page: Option<PageToken>,
        _max: u32,
    ) -> Result<MessagePage, MailError> {
        Err(mail_refused())
    }

    async fn count_messages(&self, _mb: &MailboxCtx, _q: &MessageQuery) -> Result<u64, MailError> {
        Err(mail_refused())
    }

    async fn rename_label(
        &self,
        _mb: &MailboxCtx,
        _label_id: &str,
        _new_name: &str,
    ) -> Result<(), MailError> {
        Err(mail_refused())
    }

    async fn remove_label(&self, _mb: &MailboxCtx, _label_id: &str) -> Result<(), MailError> {
        Err(mail_refused())
    }

    fn web_url(&self, _mailbox_address: &str, _id: &MessageId) -> String {
        String::new()
    }
}

#[async_trait]
impl AppFolderStore for Unused {
    async fn read(&self, _mb: &MailboxCtx) -> Result<Option<(Vec<u8>, ETag)>, MailError> {
        Err(mail_refused())
    }

    async fn write(
        &self,
        _mb: &MailboxCtx,
        _bytes: &[u8],
        _if_match: Option<&ETag>,
    ) -> Result<ETag, AppFolderError> {
        Err(AppFolderError::Mail(mail_refused()))
    }

    async fn delete(&self, _mb: &MailboxCtx) -> Result<(), MailError> {
        Err(mail_refused())
    }
}

#[async_trait]
impl HttpEgress for Unused {
    async fn one_click_post(&self, _url: &Url) -> Result<OneClickOutcome, EgressError> {
        Err(EgressError::NotPermitted)
    }

    async fn call(&self, _req: EgressRequest) -> Result<EgressResponse, EgressError> {
        Err(EgressError::NotPermitted)
    }
}

#[async_trait]
impl IdentityProvider for Unused {
    fn authorize_url(&self, _req: &AuthRequest) -> Url {
        Url::parse(UNUSED_ORIGIN).unwrap_or_else(|_| panic!("static url is valid"))
    }

    async fn exchange(
        &self,
        _code: &str,
        _verifier: &Sensitive<String>,
        _redirect_uri: &Url,
    ) -> Result<TokenSet, IdError> {
        Err(IdError::Unavailable)
    }

    async fn validate_id_token(
        &self,
        _raw: &Sensitive<String>,
        _nonce: &str,
    ) -> Result<IdClaims, IdError> {
        Err(IdError::Unavailable)
    }

    async fn refresh(&self, _refresh: &Sensitive<String>) -> Result<Sensitive<String>, IdError> {
        Err(IdError::Unavailable)
    }

    async fn revoke(&self, _token: &Sensitive<String>) -> Result<(), IdError> {
        Err(IdError::Unavailable)
    }
}

#[async_trait]
impl Secrets for Unused {
    async fn get(&self, _name: SecretName) -> Result<Sensitive<Vec<u8>>, SecretError> {
        Err(SecretError::Unavailable)
    }
}

#[async_trait]
impl CallerVerifier for Unused {
    async fn verify(
        &self,
        _bearer: &Sensitive<String>,
        _audience: &str,
    ) -> Result<VerifiedCaller, CallerAuthError> {
        Err(CallerAuthError::BadSignature)
    }
}

#[async_trait]
impl InviteMailer for Unused {
    async fn send_invite(
        &self,
        _mb: &MailboxCtx,
        _to: &EmailAddress,
        _link: &InviteLink,
    ) -> Result<(), MailError> {
        Err(mail_refused())
    }
}

#[async_trait]
impl SystemKeyService for Unused {
    async fn seal(&self, _aad: &SystemAad, _plaintext: &[u8]) -> Result<Vec<u8>, KeyError> {
        Err(KeyError::Unavailable)
    }

    async fn open(&self, _aad: &SystemAad, _ciphertext: &[u8]) -> Result<Vec<u8>, KeyError> {
        Err(KeyError::Unavailable)
    }
}
