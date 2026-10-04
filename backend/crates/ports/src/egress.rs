//! The `HttpEgress` port: the only way core code makes outbound HTTP calls.

use async_trait::async_trait;
use obs::Sensitive;
use url::Url;

/// An HTTP method.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HttpMethod {
    Get,
    Post,
    Put,
    Patch,
    Delete,
}

/// An outbound HTTP request. Header values may be bearer tokens; never logged.
pub struct EgressRequest {
    pub method: HttpMethod,
    pub url: Url,
    pub headers: Vec<(String, Sensitive<String>)>,
    pub body: Option<Vec<u8>>,
    pub timeout: std::time::Duration,
}

/// An outbound HTTP response.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EgressResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

/// The outcome of a one-click unsubscribe POST.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OneClickOutcome {
    /// 2xx.
    Accepted { status: u16 },
    /// 3xx, not followed.
    Redirected { status: u16 },
    /// 4xx or 5xx.
    Rejected { status: u16 },
    /// No full response within the timeout.
    TimedOut,
}

/// A refused IP address range.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RefusedRange {
    Unspecified,
    Loopback,
    Private,
    Cgnat,
    LinkLocal,
    Metadata,
    UniqueLocal,
    Multicast,
    Broadcast,
    Documentation,
    Benchmarking,
    Reserved,
    Translation,
}

/// An egress error.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum EgressError {
    #[error("scheme not allowed")]
    SchemeNotAllowed,
    #[error("credentials in url")]
    CredentialsInUrl,
    #[error("port not allowed")]
    PortNotAllowed,
    #[error("ip literal host")]
    IpLiteralHost,
    #[error("host not allowed")]
    HostNotAllowed,
    #[error("address refused: {0:?}")]
    AddressRefused(RefusedRange),
    #[error("permanent delete refused")]
    PermanentDeleteRefused,
    #[error("not permitted for this service")]
    NotPermitted,
    #[error("dns failure")]
    DnsFailed,
    #[error("connect failure")]
    Connect,
    #[error("tls failure")]
    Tls,
    #[error("timeout")]
    Timeout,
    #[error("response too large")]
    ResponseTooLarge,
}

/// The only way core code makes outbound HTTP calls, with SSRF checks.
#[async_trait]
pub trait HttpEgress: Send + Sync {
    async fn one_click_post(&self, url: &Url) -> Result<OneClickOutcome, EgressError>;
    async fn call(&self, req: EgressRequest) -> Result<EgressResponse, EgressError>;
}
