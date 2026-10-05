//! Per-service outbound allowlists and their predicates.

use url::Url;

/// The services that own an egress policy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Service {
    Api,
    Unsub,
    Worker,
}

/// One allowed endpoint: an exact host and the path prefix it must start with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AllowedEndpoint {
    pub host: &'static str,
    pub path_prefix: &'static str,
}

pub const API_ALLOWLIST: &[AllowedEndpoint] = &[
    AllowedEndpoint {
        host: "gmail.googleapis.com",
        path_prefix: "/gmail/v1/",
    },
    AllowedEndpoint {
        host: "www.googleapis.com",
        path_prefix: "/drive/v2/",
    },
    AllowedEndpoint {
        host: "www.googleapis.com",
        path_prefix: "/drive/v3/",
    },
    AllowedEndpoint {
        host: "www.googleapis.com",
        path_prefix: "/upload/drive/v2/",
    },
    AllowedEndpoint {
        host: "www.googleapis.com",
        path_prefix: "/upload/drive/v3/",
    },
    AllowedEndpoint {
        host: "www.googleapis.com",
        path_prefix: "/oauth2/v3/certs",
    },
    AllowedEndpoint {
        host: "oauth2.googleapis.com",
        path_prefix: "/token",
    },
    AllowedEndpoint {
        host: "oauth2.googleapis.com",
        path_prefix: "/revoke",
    },
    AllowedEndpoint {
        host: "accounts.google.com",
        path_prefix: "/.well-known/openid-configuration",
    },
    AllowedEndpoint {
        host: "us-central1-aiplatform.googleapis.com",
        path_prefix: "/v1/projects/",
    },
    AllowedEndpoint {
        host: "api.typesafe.ai",
        path_prefix: "/v1/systemone",
    },
];

pub const UNSUB_ALLOWLIST: &[AllowedEndpoint] = &[
    AllowedEndpoint {
        host: "gmail.googleapis.com",
        path_prefix: "/gmail/v1/",
    },
    AllowedEndpoint {
        host: "oauth2.googleapis.com",
        path_prefix: "/token",
    },
    AllowedEndpoint {
        host: "www.googleapis.com",
        path_prefix: "/oauth2/v3/certs", // T-701 caller check
    },
];

pub const WORKER_ALLOWLIST: &[AllowedEndpoint] = &[AllowedEndpoint {
    host: "www.googleapis.com",
    path_prefix: "/oauth2/v3/certs", // T-701 caller check
}];

/// The allowlist for a service. Exhaustive: every variant has a list.
pub fn allowlist(s: Service) -> &'static [AllowedEndpoint] {
    match s {
        Service::Api => API_ALLOWLIST,
        Service::Unsub => UNSUB_ALLOWLIST,
        Service::Worker => WORKER_ALLOWLIST,
    }
}

/// True when the URL is https, on port 443 (or unspecified), has an exactly
/// matching host and a path that starts with the endpoint's prefix.
pub fn allows(s: Service, url: &Url) -> bool {
    if url.scheme() != "https" {
        return false;
    }
    match url.port() {
        None | Some(443) => {}
        Some(_) => return false,
    }
    let Some(host) = url.host_str() else {
        return false;
    };
    let host = host.to_ascii_lowercase();
    let path = url.path();
    allowlist(s)
        .iter()
        .any(|e| e.host == host && path.starts_with(e.path_prefix))
}

/// Only the unsubscribe service may send one-click POSTs.
pub fn one_click_permitted(s: Service) -> bool {
    matches!(s, Service::Unsub)
}
