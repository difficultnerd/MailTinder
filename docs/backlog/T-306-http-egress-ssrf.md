# T-306: HttpEgress: per-service allowlist and SSRF checks

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M3 | strong | about 550 lines of code plus about 450 lines of tests | T-202b, T-307, T-708 |

**Read only these spec sections:** S6 section 3 rows T3 and T11, section 6 "One-click" bullet (`docs/specs/S6-security.md`); S4 3.3 steps 5 and 6, S4 5.7 first bullet (`docs/specs/S4-architecture.md`); S2 UN-02 AC1 and AC4 (`docs/specs/S2-v1-acceptance-criteria.md`); S10 6.1, the "SSRF on one-click targets" row of 6.2 and the paragraph after the table, and the "Egress allowlist" row of 7.2 (`docs/specs/S10-test-strategy.md`); ASVS register rows V1.3.6, V12.2.1, V12.3.1, V12.3.2, V13.2.4, V13.2.5, V15.3.2, V16.3.4 (`docs/security/asvs-l2-register.md`); `docs/backlog/T-201a-port-traits.md` ("egress.rs" block); `docs/backlog/T-702-one-click-unsubscribe-sender.md` (the mapping table, so variant meanings match). Nothing else is needed.

## Goal

The `egress` crate implements `HttpEgress` for production: every outbound HTTP call a service makes to anything other than Google Cloud platform APIs goes through it. `call` reaches only the hosts and paths on that service's allowlist. `one_click_post` (only in `unsub`) sends the RFC 8058 POST to an untrusted URL from a mail header after SSRF checks: https only, port 443, no credentials, no IP literals, no internal names; the host is resolved once, every answer is checked against the refused address ranges, the checked address is pinned for the connection (no second DNS lookup, so DNS rebinding cannot redirect it), and no redirect is ever followed.

## Files

| Action | Path | What |
| --- | --- | --- |
| Create | `backend/crates/egress/src/lib.rs` | `pub mod` list and re-exports |
| Create | `backend/crates/egress/src/ranges.rs` | `classify_ip` (pure) |
| Create | `backend/crates/egress/src/target.rs` | URL and host checks for one-click (pure) |
| Create | `backend/crates/egress/src/allowlist.rs` | `Service`, `AllowedEndpoint`, per-service constants, `allows` |
| Create | `backend/crates/egress/src/resolver.rs` | `Resolver` trait, `SystemResolver` |
| Create | `backend/crates/egress/src/client.rs` | `ProdEgress` (implements `HttpEgress`) |
| Create | `backend/crates/egress/src/test_policy.rs` | `TestOverride`, compiled only with feature `test-policy` |
| Create | `backend/crates/egress/tests/ranges_table.rs` | address table and property tests |
| Create | `backend/crates/egress/tests/one_click.rs` | integration with local servers and a fake resolver |
| Change | `backend/crates/egress/Cargo.toml` | deps `ports`, `obs`, `reqwest` (workspace: rustls, no default features), `tokio` (net), `url`, `async-trait`, `thiserror`; feature `test-policy = []`; dev `axum`, `proptest`, `testkit` |
| Change | `.semgrep/mailtinder.yml` | rule `mailtinder-tls-verification-on` |
| Change | `.semgrep/tests/mailtinder.rs` | rule test cases |

## Types and signatures

```rust
// ranges.rs
/// Allowed only for globally routable unicast addresses.
pub fn classify_ip(ip: std::net::IpAddr) -> Result<(), ports::RefusedRange>;

// target.rs
pub const ONE_CLICK_PORT: u16 = 443;
pub const MAX_URL_LEN: usize = 2048;
pub const REFUSED_NAMES: [&str; 3] = ["localhost", "metadata.google.internal", "metadata"];
pub const REFUSED_SUFFIXES: [&str; 6] = [".localhost", ".internal", ".local", ".localdomain", ".home.arpa", ".lan"];
/// Checks a one-click URL before any DNS: returns the lower-cased host without a trailing dot.
pub fn check_one_click_url(url: &Url) -> Result<String, EgressError>;

// allowlist.rs
#[derive(Clone, Copy, Debug, PartialEq, Eq)] pub enum Service { Api, Unsub, Worker }
#[derive(Clone, Copy, Debug, PartialEq, Eq)] pub struct AllowedEndpoint { pub host: &'static str, pub path_prefix: &'static str }
pub const API_ALLOWLIST: &[AllowedEndpoint] = &[
    AllowedEndpoint { host: "gmail.googleapis.com", path_prefix: "/gmail/v1/" },
    AllowedEndpoint { host: "www.googleapis.com", path_prefix: "/drive/v2/" },
    AllowedEndpoint { host: "www.googleapis.com", path_prefix: "/drive/v3/" },
    AllowedEndpoint { host: "www.googleapis.com", path_prefix: "/upload/drive/v2/" },
    AllowedEndpoint { host: "www.googleapis.com", path_prefix: "/upload/drive/v3/" },
    AllowedEndpoint { host: "www.googleapis.com", path_prefix: "/oauth2/v3/certs" },
    AllowedEndpoint { host: "oauth2.googleapis.com", path_prefix: "/token" },
    AllowedEndpoint { host: "oauth2.googleapis.com", path_prefix: "/revoke" },
    AllowedEndpoint { host: "accounts.google.com", path_prefix: "/.well-known/openid-configuration" },
    AllowedEndpoint { host: "us-central1-aiplatform.googleapis.com", path_prefix: "/v1/projects/" },
    AllowedEndpoint { host: "api.typesafe.ai", path_prefix: "/v1/systemone" },
];
pub const UNSUB_ALLOWLIST: &[AllowedEndpoint] = &[
    AllowedEndpoint { host: "gmail.googleapis.com", path_prefix: "/gmail/v1/" },
    AllowedEndpoint { host: "oauth2.googleapis.com", path_prefix: "/token" },
    AllowedEndpoint { host: "www.googleapis.com", path_prefix: "/oauth2/v3/certs" }, // T-701 caller check
];
pub const WORKER_ALLOWLIST: &[AllowedEndpoint] = &[
    AllowedEndpoint { host: "www.googleapis.com", path_prefix: "/oauth2/v3/certs" }, // T-701 caller check
];
pub fn allowlist(s: Service) -> &'static [AllowedEndpoint]; // exhaustive match, no `_`
pub fn allows(s: Service, url: &Url) -> bool;               // https, port 443 or none, exact host, path starts with prefix
pub fn one_click_permitted(s: Service) -> bool;             // Unsub only

// resolver.rs
#[async_trait::async_trait]
pub trait Resolver: Send + Sync { async fn lookup(&self, host: &str) -> Result<Vec<std::net::IpAddr>, EgressError>; }
pub struct SystemResolver; // tokio::net::lookup_host((host, 443)), deduplicated, order kept

// client.rs
pub const ONE_CLICK_BODY: &[u8] = b"List-Unsubscribe=One-Click";
pub const ONE_CLICK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10); // S6 6
pub const USER_AGENT: &str = "MailTinder-Unsubscribe/1";                               // [DEFAULT]
pub const ONE_CLICK_MAX_BODY: usize = 64 * 1024;          // read at most this, then drop the connection
pub const CALL_MAX_BODY: usize = 8 * 1024 * 1024;         // T-401 MAX_RESPONSE_BYTES
pub const CALL_MAX_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
pub struct ProdEgress { service: Service, resolver: Arc<dyn Resolver>, base_client: reqwest::Client,
                        #[cfg(feature = "test-policy")] test: Option<TestOverride> }
impl ProdEgress { pub fn new(service: Service, resolver: Arc<dyn Resolver>) -> Result<Self, EgressError>; }
impl HttpEgress for ProdEgress { /* below */ }

// test_policy.rs  (#[cfg(feature = "test-policy")])
pub struct TestOverride {
    pub allow_socket: Option<SocketAddr>,          // the one loopback socket a test may reach (testbed)
    pub allow_plain_http_to_socket: bool,
    pub extra_root_ca_pem: Option<Vec<u8>>,        // T-708 test CA
    pub host_routes: Vec<(&'static str, SocketAddr)>, // allowlisted Google host -> local fake (fake-google)
    pub one_click_timeout: std::time::Duration,    // 250 ms in tests (T-702)
}
impl ProdEgress { pub fn with_test_override(service: Service, resolver: Arc<dyn Resolver>, t: TestOverride) -> Result<Self, EgressError>; }
```

## Algorithm

### Refused address ranges (`classify_ip`)

1. IPv4, refused with the variant shown:
   - `0.0.0.0/8` Unspecified; `127.0.0.0/8` Loopback.
   - `10.0.0.0/8`, `172.16.0.0/12`, `192.168.0.0/16` Private.
   - `100.64.0.0/10` Cgnat.
   - `169.254.169.254/32` Metadata (check before the next line); `169.254.0.0/16` LinkLocal.
   - `192.0.0.0/24` Reserved (IETF protocol assignments); `192.0.2.0/24`, `198.51.100.0/24`, `203.0.113.0/24` Documentation; `198.18.0.0/15` Benchmarking; `192.88.99.0/24` Translation (6to4 relay).
   - `224.0.0.0/4` Multicast; `255.255.255.255/32` Broadcast; `240.0.0.0/4` Reserved.
   - Everything else: allowed.
2. IPv6:
   - `::/128` Unspecified; `::1/128` Loopback.
   - `::ffff:0:0/96` (IPv4-mapped): take the embedded IPv4 and apply step 1 (so `::ffff:127.0.0.1` is Loopback, `::ffff:169.254.169.254` is Metadata). Use `Ipv6Addr::to_ipv4_mapped()`, not `to_ipv4()` (which also maps `::/96`).
   - `::/96` (IPv4-compatible, deprecated), `64:ff9b::/96` and `64:ff9b:1::/48` (NAT64), `2002::/16` (6to4), `2001::/32` (Teredo) Translation.
   - `fd00:ec2::254` and `fd20:ce::254`-style addresses are covered by `fc00::/7` UniqueLocal (Google's IPv6 metadata address lives here; report the variant as UniqueLocal).
   - `fe80::/10` LinkLocal; `fec0::/10` (deprecated site-local) Private.
   - `ff00::/8` Multicast; `2001:db8::/32` Documentation; `100::/64` Reserved (discard); `2001::/23` other IETF protocol assignments Reserved.
   - Anything outside `2000::/3`: Reserved. Inside `2000::/3` and not above: allowed.
3. Write the table as data (`const` arrays of `(network, prefix_len, RefusedRange)`), checked in order with prefix masking. No string parsing of addresses at check time.

### One-click URL checks (`check_one_click_url`), before any DNS

1. `url.as_str().len() > MAX_URL_LEN`: `HostNotAllowed`.
2. Scheme must be exactly `https` (`http` gives `SchemeNotAllowed`, ASVS V12.2.1).
3. `url.username()` non-empty or `url.password()` present: `CredentialsInUrl`.
4. `url.port()` must be `None` or `Some(443)`: else `PortNotAllowed`.
5. `url.host()`: `Host::Ipv4` or `Host::Ipv6` gives `IpLiteralHost`. The `url` crate already normalises decimal (`2130706433`), octal (`0177.0.0.1`) and hex (`0x7f.1`) IPv4 forms into `Host::Ipv4` for `https`, so all of them land here. `Host::Domain(d)`: lower-case, strip one trailing `.`.
6. Refuse a name in `REFUSED_NAMES`, a name ending with any `REFUSED_SUFFIXES` entry, and a single-label name (no `.`) `[DEFAULT]`: `HostNotAllowed`. `metadata.google.internal` and its trailing-dot form are refused here by name, and again by address.

### `one_click_post(url)`

1. `one_click_permitted(self.service)` must be true, else `NotPermitted`.
2. `host = check_one_click_url(url)?`.
3. `addrs = resolver.lookup(&host)` exactly once. Empty: `DnsFailed`.
4. Check every answer with `classify_ip`. If any answer is refused, refuse the whole target with that range `[DEFAULT]` (an attacker can choose which answer a client would use). Under `TestOverride`, an answer equal to `allow_socket.ip()` with the URL's port equal to `allow_socket.port()` counts as allowed, and nothing else changes.
5. `pinned = SocketAddr::new(addrs[0], 443)` (or the test socket's port).
6. Build a fresh `reqwest::Client` for this request only: `.resolve(&host, pinned)` (pins the checked address; reqwest will not query DNS for this host), `.redirect(reqwest::redirect::Policy::none())`, `.no_proxy()` (a proxy would resolve the name itself and defeat pinning), `.https_only(true)` (unless the test override allows plain http to its socket), `.use_rustls_tls()`, `.timeout(ONE_CLICK_TIMEOUT)`, `.connect_timeout(ONE_CLICK_TIMEOUT)`, `.referer(false)`, `.user_agent(USER_AGENT)`, no cookie store, `.http1_only()` `[DEFAULT]` (simplest, widely supported). TLS verifies the certificate against the original host name (SNI and hostname check use `host`, not the IP).
7. Send `POST url` with header `Content-Type: application/x-www-form-urlencoded` and body exactly `ONE_CLICK_BODY`. No other headers: no `Cookie`, `Authorization`, `Referer`, `Origin`.
8. Result: status 200 to 299 `Accepted`; 300 to 399 `Redirected` (the `Location` is never read or followed); any other status `Rejected`. Read at most `ONE_CLICK_MAX_BODY` bytes of the body and discard them. A reqwest timeout gives `Ok(TimedOut)`; a TLS error gives `Err(Tls)`; a connect error gives `Err(Connect)`.
9. Log one structured event through T-307 (route template `egress.one_click`, outcome code, latency). Never the URL, host, address or response body. TLS failures are security events (ASVS V16.3.4).

### `call(req)`

1. `allows(self.service, &req.url)` must be true, else `HostNotAllowed` (and log a refusal event with route template `egress.call` and outcome `host_refused`).
2. Gmail permanent delete guard (INV-5 defence in depth): if the host is `gmail.googleapis.com` and the method is `Delete`, or the path ends with `/batchDelete`, return `PermanentDeleteRefused` without sending.
3. Resolve and check addresses exactly as one-click steps 3 and 4 (Google hosts resolve to public addresses; this blocks a poisoned resolver), then pin the same way.
4. Client: same settings as one-click except the timeout is `min(req.timeout, CALL_MAX_TIMEOUT)` and the body cap is `CALL_MAX_BODY` (over the cap: `ResponseTooLarge`). Headers are taken from `req.headers` (values exposed only when building the request). Redirects are never followed.
5. Under `TestOverride.host_routes`, an allowlisted host is sent to the mapped local socket over plain http instead (the allowlist check still runs on the original URL first).

### Semgrep (ASVS V12.3.2)

`mailtinder-tls-verification-on` matches `danger_accept_invalid_certs`, `danger_accept_invalid_hostnames`, `.dangerous()` and `NoCertificateVerification` anywhere under `backend/`, with rule tests.

## Acceptance criteria

| ID | Behaviour (one line) |
| --- | --- |
| UN-02 AC1 | The one-click request is one HTTPS POST with body `List-Unsubscribe=One-Click` and no cookies or credentials |
| UN-02 AC4 | No redirects are followed; private, loopback, link-local, CGNAT and metadata targets are refused before connecting; the resolved address is pinned; https only; 10-second timeout |
| V1.3.6 | One-click targets are validated against scheme, port, host and address rules before use |
| V12.2.1 | `http` targets are refused; no fallback to plain HTTP |
| V12.3.1 | Every outbound call uses TLS (rustls) |
| V13.2.4 | Each service has a fixed allowlist of endpoints |
| V13.2.5 | Any other host is refused before connecting, in production configuration |
| V15.3.2 | Redirects are off for every outbound call |
| V16.3.4 | Backend TLS failures are logged as security events |
| INV-5 | A Gmail permanent delete call cannot leave the process |

## Tests that must pass

- `asvs_v1_3_6_refused_ranges_table` (unit, `ranges_table.rs`): one row per range above with a sample address, including `169.254.169.254`, `100.64.0.1`, `::1`, `fe80::1`, `fd20:ce::254`, `::ffff:127.0.0.1`, `::ffff:169.254.169.254`, `::ffff:10.0.0.1`, `64:ff9b::7f00:1`, `2002:7f00:1::`, `2001:0:4136:e378::1`, plus allowed samples `93.184.216.34` (never contacted) and `2606:4700::1`.
- `asvs_v1_3_6_every_address_in_refused_v4_ranges_is_refused` (property, `proptest`: random host bits inside each IPv4 range).
- `asvs_v1_3_6_ipv4_mapped_matches_ipv4_verdict` (property: for every IPv4 `a`, `classify_ip(a)` and `classify_ip(a.to_ipv6_mapped())` agree).
- `asvs_v1_3_6_url_checks_table` (unit: `http://`, `https://user:pw@host/`, port 8443, `https://2130706433/`, `https://0177.0.0.1/`, `https://0x7f.1/`, `https://[::1]/`, `https://localhost/`, `https://metadata.google.internal./`, `https://metadata/`, `https://svc.internal/`, `https://printer.local/`, an over-long URL; each gives its error).
- `un_02_ac4_refuses_each_forbidden_resolution` (integration with a fake resolver mapping `.test` names to each forbidden address; zero connections, error `AddressRefused(range)`).
- `un_02_ac4_any_refused_answer_refuses_target` (integration: resolver answers one public and one private address).
- `un_02_ac4_dns_rebinding_is_pinned` (integration: test override allows one local plain-http server; a counting fake resolver answers that server's address the first time and `10.0.0.1` afterwards; the POST reaches the server and the resolver was called exactly once).
- `un_02_ac4_redirect_not_followed` (integration: local server answers 302 with `Location` to a second local server; result `Redirected`; second server has zero hits).
- `un_02_ac4_timeout_is_timed_out` (integration: server never answers; test override timeout 250 ms; result `TimedOut`).
- `un_02_ac1_request_shape` (integration: server records method `POST`, body exactly `List-Unsubscribe=One-Click`, `Content-Type`, the fixed `User-Agent`, and no `Cookie`, `Authorization`, `Referer` or `Origin`).
- `asvs_v12_2_1_http_target_refused` (unit).
- `asvs_v12_3_1_production_client_is_https_only` (unit: `ProdEgress::new` without override refuses an `http://` allowlisted URL with `SchemeNotAllowed`).
- `asvs_v13_2_4_allowlists_table` (unit: each service's allowed and refused URL samples, including `unsub` refusing `https://www.googleapis.com/drive/v3/files` and `api` refusing `https://example.com/`).
- `asvs_v13_2_5_unlisted_host_refused_before_connect` (integration: resolver records zero lookups for a refused host).
- `asvs_v15_3_2_call_follows_no_redirect` (integration).
- `asvs_v16_3_4_tls_failure_is_error_and_logged` (integration: a local HTTPS server with a self-signed certificate made with `rcgen` as a dev-dependency (or T-708's untrusted listener if merged); result `Err(Tls)` and T-307's capture holds one `egress_tls_failure` security event).
- `inv_5_egress_refuses_gmail_delete_and_batch_delete` (unit).
- `one_click_not_permitted_for_api_or_worker` (unit).
- Semgrep rule test for `mailtinder-tls-verification-on`.

## Edge cases and traps

- Pin with `ClientBuilder::resolve`; do not connect to the IP in the URL, which breaks TLS name checks and SNI.
- Build a new client per one-click request; a shared client with `.resolve()` would pin one host for everyone.
- `.no_proxy()` is required: with `HTTPS_PROXY` set, reqwest would hand the host name to the proxy, skipping both the address check and the pin.
- `Ipv6Addr::to_ipv4()` treats `::1` as `0.0.0.1`; use `to_ipv4_mapped()` for the mapped range and handle `::/96` separately.
- Check `169.254.169.254` before `169.254.0.0/16` so the variant is `Metadata` (the reason code shown to the user is the same either way).
- Do not trust `url.host_str()` for IP detection; match on `url.host()`.
- A 3xx is an outcome (`Ok(Redirected)`), not an error, so T-702 can raise Needs Attention without retrying (S2 UN-02 AC4).
- Never retry inside egress; Cloud Tasks is the only retry layer (S3).
- The `test-policy` feature must never be enabled by the `api`, `unsub` or `worker` `[dependencies]`; tests enable it under `[dev-dependencies]`. Add a comment in `egress/Cargo.toml`, and tell the reviewer that `--all-features` in CI compiles it for tests only.
- `reqwest` must use rustls with the `ring` provider; check `cargo deny check licenses` (no `aws-lc-sys`).
- `matches!` and `match` on `Service`, `RefusedRange`, `EgressError` and `Provider` must not use `_`.
- Do not log URLs, hosts, resolved addresses or response bodies (S5 logs; XC-01). The privacy Semgrep log rule also flags `url` and `host` words in log calls.
- T-702's test `asvs_v13_2_4_unsub_never_reaches_drive` should assert `!allows(Service::Unsub, drive_url)` rather than "no Drive host in the constant", because `www.googleapis.com` is allowed for the certs path only.

## Out of scope

- The unsubscribe runner, Needs Attention items and retries (T-701, T-702).
- The testbed server and its SSRF host table (T-708).
- Platform API calls (Firestore, KMS, Cloud Tasks, Secret Manager) and the metadata token, which use `GcpHttp` (T-301), never this crate.
- v2 page handler egress and Direct VPC egress rules.

## Security review checklist

- Every refused range in the Algorithm is in the table, including CGNAT `100.64.0.0/10`, `169.254.169.254`, IPv6 loopback, link-local, unique local (Google IPv6 metadata), NAT64, 6to4, Teredo and IPv4-mapped IPv6 (embedded address re-checked).
- IP literals in any form (decimal, octal, hex, IPv6) are refused; `metadata.google.internal`, `metadata`, `localhost` and internal suffixes are refused by name before DNS.
- DNS is resolved once; all answers are checked; the connection is pinned with `resolve()`; `no_proxy()` is set; TLS hostname verification is on.
- Redirect policy is `none` on every client; 3xx is never followed and never retried.
- https only, port 443 only, no credentials in URLs, fixed body, fixed headers, 10-second timeout, response body capped.
- Per-service allowlists match S4 5.7 plus the documented additions (certs path for `unsub` and `worker`); `unsub` cannot reach Drive; `one_click_post` works only in `unsub`.
- The test override exists only behind the `test-policy` feature and allows exactly one socket.
- No URL, host, address, token or body reaches a log; TLS failures produce a security event.
- The Gmail delete guard refuses `DELETE` and `batchDelete`.

## Done when

- The tests above pass and every required check is green (S10 10.1).
- Definition of done in S10 10.4.
- A strong-model review has signed off the checklist above in the pull request.
