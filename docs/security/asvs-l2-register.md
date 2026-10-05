# OWASP ASVS 5.0 Level 2 Register

Status: DRAFT. 3 October 2026. Generated from the official ASVS 5.0.0 requirement list (Level 1 and Level 2 rows, 253 requirements), then mapped section by section.

How to use:

- Every requirement has an applicability, a section control and, per row, `Control`, `Location` and `Verify` (spec audit M11, 3 October 2026). `Location` names the planned module under `backend/src/`, `app/`, `firebase.json` or Terraform. `Verify` uses the S10 7.1 methods: `test` (named `asvs_<row>_*`), `semgrep`, `ci`, `dast` or `review`. Rows marked v2 scope concern Microsoft or the page handler, both deferred to v2 (James, 3 October 2026).
- A feature task that touches a requirement cites its ID, and the task is not done until the verification passes.
- **Status values:** Planned (mapped, not built), Built (control in code, verification failing or absent), Verified (verification passing in CI or recorded review), N/A (with reason).
- The CASA assessment maps to ASVS; translate IDs at assessment time if CASA still uses ASVS 4.0.

Source: OWASP Application Security Verification Standard 5.0.0, licensed CC BY-SA 4.0. Requirement text is reproduced unchanged.

## Summary by chapter


| Chapter | Requirements | N/A |
| --- | --- | --- |
| V1 Encoding and Sanitization | 27 | 5 |
| V2 Validation and Business Logic | 11 | 0 |
| V3 Web Frontend Security | 19 | 0 |
| V4 API and Web Service | 10 | 6 |
| V5 File Handling | 9 | 5 |
| V6 Authentication | 35 | 24 |
| V7 Session Management | 18 | 0 |
| V8 Authorization | 7 | 0 |
| V9 Self-contained Tokens | 7 | 0 |
| V10 OAuth and OIDC | 29 | 18 |
| V11 Cryptography | 14 | 2 |
| V12 Secure Communication | 9 | 1 |
| V13 Configuration | 13 | 0 |
| V14 Data Protection | 9 | 0 |
| V15 Secure Coding and Architecture | 13 | 0 |
| V16 Security Logging and Error Handling | 16 | 0 |
| V17 WebRTC | 7 | 7 |


## V1.1 Encoding and Sanitization Architecture

**Planned control:** Single decode point in the api request layer; output encoding in Flutter text widgets

**Verification:** Unit tests; code review

| ID | L | Requirement | Control | Location | Verify | Status |
| --- | --- | --- | --- | --- | --- | --- |
| V1.1.1 | 2 | Verify that input is decoded or unescaped into a canonical form only once, it is only decoded when encoded data in that form is expected, and that this is done before processing the input further, for example it is not performed after input validation or sanitization. | Single decode point in the request layer | backend/http | test `asvs_v1_1_1_*` | Planned |
| V1.1.2 | 2 | Verify that the application performs output encoding and escaping either as a final step before being used by the interpreter for which it is intended or by the interpreter itself. | Encode at output: Flutter `Text` widgets; JSON via serde | backend/http, app/ | test `asvs_v1_1_2_*` | Planned |

## V1.2 Injection Prevention

**Planned control:** Firestore and provider SDK calls built with typed parameters; no string-built queries, shell calls or template evaluation. V1.2.6 to V1.2.8 are N/A (no LDAP, XPath or LaTeX)

**Verification:** Semgrep rules; unit tests

| ID | L | Requirement | Control | Location | Verify | Status |
| --- | --- | --- | --- | --- | --- | --- |
| V1.2.1 | 1 | Verify that output encoding for an HTTP response, HTML document, or XML document is relevant for the context required, such as encoding the relevant characters for HTML elements, HTML attributes, HTML comments, CSS, or HTTP header fields, to avoid changing the message or document structure. | serde JSON output; typed header builders; CSV cells formula-escaped | backend/http, backend/admin | test `asvs_v1_2_1_*` | Planned |
| V1.2.2 | 1 | Verify that when dynamically building URLs, untrusted data is encoded according to its context (e.g., URL encoding or base64url encoding for query or path parameters). Ensure that only safe URL protocols are permitted (e.g., disallow javascript: or data:). | URLs built with encoders; only `https` and `mailto` schemes accepted | backend/unsub | test `asvs_v1_2_2_*` | Planned |
| V1.2.3 | 1 | Verify that output encoding or escaping is used when dynamically building JavaScript content (including JSON), to avoid changing the message or document structure (to avoid JavaScript and JSON injection). | JSON only via serde, never string-built | backend/http | semgrep | Planned |
| V1.2.4 | 1 | Verify that data selection or database queries (e.g., SQL, HQL, NoSQL, Cypher) use parameterized queries, ORMs, entity frameworks, or are otherwise protected from SQL Injection and other database injection attacks. This is also relevant when writing stored procedures. | Typed Firestore queries; no string-built queries | backend/store | semgrep | Planned |
| V1.2.5 | 1 | Verify that the application protects against OS command injection and that operating system calls use parameterized OS queries or use contextual command line output encoding. | No shell or OS command calls | backend/ | semgrep | Planned |
| V1.2.6 | 2 | Verify that the application protects against LDAP injection vulnerabilities, or that specific security controls to prevent LDAP injection have been implemented. | N/A: no LDAP anywhere in the system | - | - | N/A |
| V1.2.7 | 2 | Verify that the application is protected against XPath injection attacks by using query parameterization or precompiled queries. | N/A: no XML or XPath processing | - | - | N/A |
| V1.2.8 | 2 | Verify that LaTeX processors are configured securely (such as not using the "--shell-escape" flag) and an allowlist of commands is used to prevent LaTeX injection attacks. | N/A: no LaTeX processing | - | - | N/A |
| V1.2.9 | 2 | Verify that the application escapes special characters in regular expressions (typically using a backslash) to prevent them from being misinterpreted as metacharacters. | Untrusted text escaped before use in a regular expression | backend/ | semgrep | Planned |

## V1.3 Sanitization

**Planned control:** Mail content rendered as plain text only; HTML stripped server side with an allowlist sanitiser; mailto and URL parsing with strict parsers. V1.3.8 and V1.3.9 are N/A (no JNDI, no memcache)

**Verification:** Unit tests with hostile mail fixtures

| ID | L | Requirement | Control | Location | Verify | Status |
| --- | --- | --- | --- | --- | --- | --- |
| V1.3.1 | 1 | Verify that all untrusted HTML input from WYSIWYG editors or similar is sanitized using a well-known and secure HTML sanitization library or framework feature. | Mail HTML stripped server side with an allowlist sanitiser; plain text only | backend/feed | test `asvs_v1_3_1_*` | Planned |
| V1.3.2 | 1 | Verify that the application avoids the use of eval() or other dynamic code execution features such as Spring Expression Language (SpEL). Where there is no alternative, any user input being included must be sanitized before being executed. | No eval or dynamic code execution | backend/, app/ | semgrep | Planned |
| V1.3.3 | 2 | Verify that data being passed to a potentially dangerous context is sanitized beforehand to enforce safety measures, such as only allowing characters which are safe for this context and trimming input which is too long. | Length and character limits before provider, model and send calls | backend/feed, backend/unsub | test `asvs_v1_3_3_*` | Planned |
| V1.3.4 | 2 | Verify that user-supplied Scalable Vector Graphics (SVG) scriptable content is validated or sanitized to contain only tags and attributes (such as draw graphics) that are safe for the application, e.g., do not contain scripts and foreignObject. | No SVG from mail or users is rendered | app/ | review | Planned |
| V1.3.5 | 2 | Verify that the application sanitizes or disables user-supplied scriptable or expression template language content, such as Markdown, CSS or XSL stylesheets, BBCode, or similar. | No Markdown, CSS or template rendering of mail content | app/ | review | Planned |
| V1.3.6 | 2 | Verify that the application protects against Server-side Request Forgery (SSRF) attacks, by validating untrusted data against an allowlist of protocols, domains, paths and ports and sanitizing potentially dangerous characters before using the data to call another service. | v1 `unsub` one-click: https only, host resolved, private, loopback, link-local, CGNAT and metadata ranges refused, resolved IP pinned; `HttpEgress` allowlist. Page handler checks are v2 scope | backend/egress, backend/unsub | test `asvs_v1_3_6_*` | Planned |
| V1.3.7 | 2 | Verify that the application protects against template injection attacks by not allowing templates to be built based on untrusted input. Where there is no alternative, any untrusted input being included dynamically during template creation must be sanitized or strictly validated. | No server-side templates built from input | backend/ | semgrep | Planned |
| V1.3.8 | 2 | Verify that the application appropriately sanitizes untrusted input before use in Java Naming and Directory Interface (JNDI) queries and that JNDI is configured securely to prevent JNDI injection attacks. | N/A: Rust and Dart only; no Java, no JNDI | - | - | N/A |
| V1.3.9 | 2 | Verify that the application sanitizes content before it is sent to memcache to prevent injection attacks. | N/A: no memcache; Firestore only | - | - | N/A |
| V1.3.10 | 2 | Verify that format strings which might resolve in an unexpected or malicious way when used are sanitized before being processed. | Format strings are compile-time literals | backend/ | ci | Planned |
| V1.3.11 | 2 | Verify that the application sanitizes user input before passing to mail systems to protect against SMTP or IMAP injection. | Strict mailto parsing; CR and LF rejected; no `cc` or `bcc` | backend/unsub | test `asvs_v1_3_11_*` | Planned |

## V1.4 Memory, String, and Unmanaged Code

**Planned control:** Rust with #![forbid(unsafe_code)] in all crates

**Verification:** Clippy and CI grep for unsafe

| ID | L | Requirement | Control | Location | Verify | Status |
| --- | --- | --- | --- | --- | --- | --- |
| V1.4.1 | 2 | Verify that the application uses memory-safe string, safer memory copy and pointer arithmetic to detect or prevent stack, buffer, or heap overflows. | `#![forbid(unsafe_code)]` in all crates | backend/ | ci | Planned |
| V1.4.2 | 2 | Verify that sign, range, and input validation techniques are used to prevent integer overflows. | Checked arithmetic on external counts; Clippy arithmetic lints | backend/ | ci | Planned |
| V1.4.3 | 2 | Verify that dynamically allocated memory and resources are released, and that references or pointers to freed memory are removed or set to null to prevent dangling pointers and use-after-free vulnerabilities. | Rust ownership; no `unsafe` | backend/ | ci | Planned |

## V1.5 Safe Deserialization

**Planned control:** serde with strict typed structs, deny_unknown_fields on external input

**Verification:** Unit tests

| ID | L | Requirement | Control | Location | Verify | Status |
| --- | --- | --- | --- | --- | --- | --- |
| V1.5.1 | 1 | Verify that the application configures XML parsers to use a restrictive configuration and that unsafe features such as resolving external entities are disabled to prevent XML eXternal Entity (XXE) attacks. | No XML parser in the dependency tree (cargo-deny ban) | backend/deny.toml | ci | Planned |
| V1.5.2 | 2 | Verify that deserialization of untrusted data enforces safe input handling, such as using an allowlist of object types or restricting client-defined object types, to prevent deserialization attacks. Deserialization mechanisms that are explicitly defined as insecure must not be used with untrusted input. | serde typed structs with `deny_unknown_fields` on external input | backend/http | test `asvs_v1_5_2_*` | Planned |

## V2.1 Validation and Business Logic Documentation

**Planned control:** Validation rules and business limits documented in S2 and S3

**Verification:** Review

| ID | L | Requirement | Control | Location | Verify | Status |
| --- | --- | --- | --- | --- | --- | --- |
| V2.1.1 | 1 | Verify that the application's documentation defines input validation rules for how to check the validity of data items against an expected structure. This could be common data formats such as credit card numbers, email addresses, telephone numbers, or it could be an internal data format. | Field rules in the S7 OpenAPI schema | docs/specs/S7 | review | Planned |
| V2.1.2 | 2 | Verify that the application's documentation defines how to validate the logical and contextual consistency of combined data items, such as checking that suburb and ZIP code match. | Combined checks in S2 and S3 (job, invite and session states) | docs/specs/S2, S3 | review | Planned |
| V2.1.3 | 2 | Verify that expectations for business logic limits and validations are documented, including both per-user and globally across the application. | Per-user and global limits in the S7 rate-limit table | docs/specs/S7 | review | Planned |

## V2.2 Input Validation

**Planned control:** Typed request validation at the api boundary

**Verification:** Unit tests

| ID | L | Requirement | Control | Location | Verify | Status |
| --- | --- | --- | --- | --- | --- | --- |
| V2.2.1 | 1 | Verify that input is validated to enforce business or functional expectations for that input. This should either use positive validation against an allow list of values, patterns, and ranges, or be based on comparing the input to an expected structure and logical limits according to predefined rules. For L1, this can focus on input which is used to make specific business or security decisions. For L2 and up, this should apply to all input. | Typed validation at the api boundary; allowlists for enums | backend/http | test `asvs_v2_2_1_*` | Planned |
| V2.2.2 | 1 | Verify that the application is designed to enforce input validation at a trusted service layer. While client-side validation improves usability and should be encouraged, it must not be relied upon as a security control. | Validation server side; client checks are cosmetic | backend/http | test `asvs_v2_2_2_*` | Planned |
| V2.2.3 | 2 | Verify that the application ensures that combinations of related data items are reasonable according to the pre-defined rules. | Cross-field checks in the domain layer (mailbox owner, job state) | backend/domain | test `asvs_v2_2_3_*` | Planned |

## V2.3 Business Logic Security

**Planned control:** State machines in S3 enforced server side (jobs, invites, undo); per-user limits on unsubscribes and sends

**Verification:** Integration tests per state transition

| ID | L | Requirement | Control | Location | Verify | Status |
| --- | --- | --- | --- | --- | --- | --- |
| V2.3.1 | 1 | Verify that the application will only process business logic flows for the same user in the expected sequential step order and without skipping steps. | S3 state machines enforced server side | backend/domain | test `asvs_v2_3_1_*` | Planned |
| V2.3.2 | 2 | Verify that business logic limits are implemented per the application's documentation to avoid business logic flaws being exploited. | Per-user caps on unsubscribes, sends and actions | backend/actions, backend/unsub | test `asvs_v2_3_2_*` | Planned |
| V2.3.3 | 2 | Verify that transactions are being used at the business logic level such that either a business logic operation succeeds in its entirety or it is rolled back to the previous correct state. | Firestore transactions for multi-document changes | backend/store | test `asvs_v2_3_3_*` | Planned |
| V2.3.4 | 2 | Verify that business logic level locking mechanisms are used to ensure that limited quantity resources (such as theater seats or delivery slots) cannot be double-booked by manipulating the application's logic. | Idempotency keys; job and invite transitions in transactions | backend/jobs, backend/auth | test `asvs_v2_3_4_*` | Planned |

## V2.4 Anti-automation

**Planned control:** Rate limits per user and per IP on sign-in, invite requests, unsubscribe and send

**Verification:** Integration tests

| ID | L | Requirement | Control | Location | Verify | Status |
| --- | --- | --- | --- | --- | --- | --- |
| V2.4.1 | 2 | Verify that anti-automation controls are in place to protect against excessive calls to application functions that could lead to data exfiltration, garbage-data creation, quota exhaustion, rate-limit breaches, denial-of-service, or overuse of costly resources. | Rate limits per user and per IP (S7 table) | backend/http | test `asvs_v2_4_1_*` | Planned |

## V3.2 Unintended Content Interpretation

**Planned control:** Correct Content-Type and nosniff on every response

**Verification:** Header tests

| ID | L | Requirement | Control | Location | Verify | Status |
| --- | --- | --- | --- | --- | --- | --- |
| V3.2.1 | 1 | Verify that security controls are in place to prevent browsers from rendering content or functionality in HTTP responses in an incorrect context (e.g., when an API, a user-uploaded file or other resource is requested directly). Possible controls could include: not serving the content unless HTTP request header fields (such as Sec-Fetch-\*) indicate it is the correct context, using the sandbox directive of the Content-Security-Policy header field or using the attachment disposition type in the Content-Disposition header field. | JSON responses with nosniff; CSV served as attachment | backend/http | test `asvs_v3_2_1_*` | Planned |
| V3.2.2 | 1 | Verify that content intended to be displayed as text, rather than rendered as HTML, is handled using safe rendering functions (such as createTextNode or textContent) to prevent unintended execution of content such as HTML or JavaScript. | Mail shown with Flutter `Text`, never as HTML | app/ | test `asvs_v3_2_2_*` | Planned |

## V3.3 Cookie Setup

**Planned control:** Session cookie named `__session` (Firebase Hosting forwards no other cookie) with Secure, HttpOnly, SameSite=Lax, Path=/ and no Domain attribute. This deviates from V3.3.1 and V3.3.3 (prefix requirements); the prefix's protections are set by attribute instead. Deviation accepted by James on 3 October 2026 for the trial only. Mitigations: HSTS with includeSubDomains, no untrusted hosts under the domain, session rotation at sign-in, CSRF token bound to the session. The name also lacks the `__Secure-` fallback prefix that V3.3.1 asks for; the same load balancer change removes that gap. Removal: when the pre-production load balancer (Cloud Armor decision) routes `/api` directly to Cloud Run, switch to `__Host-session`.

**Verification:** Header tests

| ID | L | Requirement | Control | Location | Verify | Status |
| --- | --- | --- | --- | --- | --- | --- |
| V3.3.1 | 1 | Verify that cookies have the 'Secure' attribute set, and if the '\__Host-' prefix is not used for the cookie name, the '__Secure-' prefix must be used for the cookie name. | `Secure` set; name `__session` has neither `__Host-` nor `__Secure-` prefix (deviation) | backend/session | test `asvs_v3_3_1_*` | Accepted deviation (trial only) |
| V3.3.2 | 2 | Verify that each cookie's 'SameSite' attribute value is set according to the purpose of the cookie, to limit exposure to user interface redress attacks and browser-based request forgery attacks, commonly known as cross-site request forgery (CSRF). | `SameSite=Lax` on `__session` | backend/session | test `asvs_v3_3_2_*` | Planned |
| V3.3.3 | 2 | Verify that cookies have the '__Host-' prefix for the cookie name unless they are explicitly designed to be shared with other hosts. | No `__Host-` prefix (deviation); attributes asserted instead | backend/session | test `asvs_v3_3_3_*` | Accepted deviation (trial only) |
| V3.3.4 | 2 | Verify that if the value of a cookie is not meant to be accessible to client-side scripts (such as a session token), the cookie must have the 'HttpOnly' attribute set and the same value (e. g. session token) must only be transferred to the client via the 'Set-Cookie' header field. | `HttpOnly`; session ID sent only in `Set-Cookie` | backend/session | test `asvs_v3_3_4_*` | Planned |

## V3.4 Browser Security Mechanism Headers

**Planned control:** CSP with Trusted Types, HSTS, frame-ancestors none, Referrer-Policy, Permissions-Policy set in Firebase Hosting and api

**Verification:** Header tests; CSP spike

| ID | L | Requirement | Control | Location | Verify | Status |
| --- | --- | --- | --- | --- | --- | --- |
| V3.4.1 | 1 | Verify that a Strict-Transport-Security header field is included on all responses to enforce an HTTP Strict Transport Security (HSTS) policy. A maximum age of at least 1 year must be defined, and for L2 and up, the policy must apply to all subdomains as well. | HSTS one year with `includeSubDomains` | firebase.json, backend/http | test `asvs_v3_4_1_*` | Planned |
| V3.4.2 | 1 | Verify that the Cross-Origin Resource Sharing (CORS) Access-Control-Allow-Origin header field is a fixed value by the application, or if the Origin HTTP request header field value is used, it is validated against an allowlist of trusted origins. When 'Access-Control-Allow-Origin: *' needs to be used, verify that the response does not include any sensitive information. | No CORS headers sent (same origin) | backend/http | test `asvs_v3_4_2_*` | Planned |
| V3.4.3 | 2 | Verify that HTTP responses include a Content-Security-Policy response header field which defines directives to ensure the browser only loads and executes trusted content or resources, in order to limit execution of malicious JavaScript. As a minimum, a global policy must be used which includes the directives object-src 'none' and base-uri 'none' and defines either an allowlist or uses nonces or hashes. For an L3 application, a per-response policy with nonces or hashes must be defined. | CSP with `object-src 'none'`, `base-uri 'none'`, Trusted Types | firebase.json | test `asvs_v3_4_3_*` | Planned |
| V3.4.4 | 2 | Verify that all HTTP responses contain an 'X-Content-Type-Options: nosniff' header field. This instructs browsers not to use content sniffing and MIME type guessing for the given response, and to require the response's Content-Type header field value to match the destination resource. For example, the response to a request for a style is only accepted if the response's Content-Type is 'text/css'. This also enables the use of the Cross-Origin Read Blocking (CORB) functionality by the browser. | `X-Content-Type-Options: nosniff` on every response | firebase.json, backend/http | test `asvs_v3_4_4_*` | Planned |
| V3.4.5 | 2 | Verify that the application sets a referrer policy to prevent leakage of technically sensitive data to third-party services via the 'Referer' HTTP request header field. This can be done using the Referrer-Policy HTTP response header field or via HTML element attributes. Sensitive data could include path and query data in the URL, and for internal non-public applications also the hostname. | `Referrer-Policy: no-referrer` | firebase.json, backend/http | test `asvs_v3_4_5_*` | Planned |
| V3.4.6 | 2 | Verify that the web application uses the frame-ancestors directive of the Content-Security-Policy header field for every HTTP response to ensure that it cannot be embedded by default and that embedding of specific resources is allowed only when necessary. Note that the X-Frame-Options header field, although supported by browsers, is obsolete and may not be relied upon. | CSP `frame-ancestors 'none'` | firebase.json, backend/http | test `asvs_v3_4_6_*` | Planned |

## V3.5 Browser Origin Separation

**Planned control:** Strict CORS (same origin, no CORS headers sent); CSRF protection on every `POST`, `PATCH` and `DELETE` by synchroniser token (`X-CSRF-Token`, bound to the session) plus `Origin` check, on top of SameSite=Lax (S6 T8, S7 3.2)

**Verification:** Integration tests

| ID | L | Requirement | Control | Location | Verify | Status |
| --- | --- | --- | --- | --- | --- | --- |
| V3.5.1 | 1 | Verify that, if the application does not rely on the CORS preflight mechanism to prevent disallowed cross-origin requests to use sensitive functionality, these requests are validated to ensure they originate from the application itself. This may be done by using and validating anti-forgery tokens or requiring extra HTTP header fields that are not CORS-safelisted request-header fields. This is to defend against browser-based request forgery attacks, commonly known as cross-site request forgery (CSRF). | Synchroniser token (`X-CSRF-Token`) plus `Origin` check on unsafe methods, on top of SameSite=Lax | backend/http | test `asvs_v3_5_1_*` | Planned |
| V3.5.2 | 1 | Verify that, if the application relies on the CORS preflight mechanism to prevent disallowed cross-origin use of sensitive functionality, it is not possible to call the functionality with a request which does not trigger a CORS-preflight request. This may require checking the values of the 'Origin' and 'Content-Type' request header fields or using an extra header field that is not a CORS-safelisted header-field. | Preflight not relied on; CSRF token and JSON `Content-Type` required | backend/http | test `asvs_v3_5_2_*` | Planned |
| V3.5.3 | 1 | Verify that HTTP requests to sensitive functionality use appropriate HTTP methods such as POST, PUT, PATCH, or DELETE, and not methods defined by the HTTP specification as "safe" such as HEAD, OPTIONS, or GET. Alternatively, strict validation of the Sec-Fetch-* request header fields can be used to ensure that the request did not originate from an inappropriate cross-origin call, a navigation request, or a resource load (such as an image source) where this is not expected. | `GET` never changes state; changes use `POST`, `PATCH`, `DELETE` | backend/http | test `asvs_v3_5_3_*` | Planned |
| V3.5.4 | 2 | Verify that separate applications are hosted on different hostnames to leverage the restrictions provided by same-origin policy, including how documents or scripts loaded by one origin can interact with resources from another origin and hostname-based restrictions on cookies. | App and API on one origin; no other apps on the hostname | firebase.json | review | Planned |
| V3.5.5 | 2 | Verify that messages received by the postMessage interface are discarded if the origin of the message is not trusted, or if the syntax of the message is invalid. | No `postMessage` listeners in the app | app/ | review | Planned |

## V3.7 Other Browser Security Considerations

**Planned control:** No third-party scripts; subresource integrity where any external asset is used

**Verification:** Build check

| ID | L | Requirement | Control | Location | Verify | Status |
| --- | --- | --- | --- | --- | --- | --- |
| V3.7.1 | 2 | Verify that the application only uses client-side technologies which are still supported and considered secure. Examples of technologies which do not meet this requirement include NSAPI plugins, Flash, Shockwave, ActiveX, Silverlight, NACL, or client-side Java applets. | Flutter web only; no plugins | app/ | review | Planned |
| V3.7.2 | 2 | Verify that the application will only automatically redirect the user to a different hostname or domain (which is not controlled by the application) where the destination appears on an allowlist. | No open redirects; OAuth redirects only to allowlisted provider endpoints | backend/auth | test `asvs_v3_7_2_*` | Planned |

## V4.1 Generic Web Service Security

**Planned control:** JSON API with explicit content types; no sensitive data in URLs

**Verification:** Integration tests

| ID | L | Requirement | Control | Location | Verify | Status |
| --- | --- | --- | --- | --- | --- | --- |
| V4.1.1 | 1 | Verify that every HTTP response with a message body contains a Content-Type header field that matches the actual content of the response, including the charset parameter to specify safe character encoding (e.g., UTF-8, ISO-8859-1) according to IANA Media Types, such as "text/", "/+xml" and "/xml". | `Content-Type` with charset on every response | backend/http | test `asvs_v4_1_1_*` | Planned |
| V4.1.2 | 2 | Verify that only user-facing endpoints (intended for manual web-browser access) automatically redirect from HTTP to HTTPS, while other services or endpoints do not implement transparent redirects. This is to avoid a situation where a client is erroneously sending unencrypted HTTP requests, but since the requests are being automatically redirected to HTTPS, the leakage of sensitive data goes undiscovered. | `api` serves HTTPS only, no HTTP redirect | backend/http | dast | Planned |
| V4.1.3 | 2 | Verify that any HTTP header field used by the application and set by an intermediary layer, such as a load balancer, a web proxy, or a backend-for-frontend service, cannot be overridden by the end-user. Example headers might include X-Real-IP, X-Forwarded-*, or X-User-ID. | Client IP read only from the trusted front-end header position | backend/http | test `asvs_v4_1_3_*` | Planned |

## V4.2 HTTP Message Structure Validation

**Planned control:** HTTP parsing by the framework; request size limits

**Verification:** Integration tests

| ID | L | Requirement | Control | Location | Verify | Status |
| --- | --- | --- | --- | --- | --- | --- |
| V4.2.1 | 2 | Verify that all application components (including load balancers, firewalls, and application servers) determine boundaries of incoming HTTP messages using the appropriate mechanism for the HTTP version to prevent HTTP request smuggling. In HTTP/1.x, if a Transfer-Encoding header field is present, the Content-Length header must be ignored per RFC 2616. When using HTTP/2 or HTTP/3, if a Content-Length header field is present, the receiver must ensure that it is consistent with the length of the DATA frames. | Message framing by Google front end and hyper | Cloud Run, Firebase Hosting | review | Planned |

## V4.3 GraphQL

**Not applicable:** No GraphQL API.

| ID | L | Requirement | Control | Location | Verify | Status |
| --- | --- | --- | --- | --- | --- | --- |
| V4.3.1 | 2 | Verify that a query allowlist, depth limiting, amount limiting, or query cost analysis is used to prevent GraphQL or data layer expression Denial of Service (DoS) as a result of expensive, nested queries. | N/A: see section note | - | - | N/A |
| V4.3.2 | 2 | Verify that GraphQL introspection queries are disabled in the production environment unless the GraphQL API is meant to be used by other parties. | N/A: see section note | - | - | N/A |

## V4.4 WebSocket

**Not applicable:** No WebSockets; the API is request and response only.

| ID | L | Requirement | Control | Location | Verify | Status |
| --- | --- | --- | --- | --- | --- | --- |
| V4.4.1 | 1 | Verify that WebSocket over TLS (WSS) is used for all WebSocket connections. | N/A: see section note | - | - | N/A |
| V4.4.2 | 2 | Verify that, during the initial HTTP WebSocket handshake, the Origin header field is checked against a list of origins allowed for the application. | N/A: see section note | - | - | N/A |
| V4.4.3 | 2 | Verify that, if the application's standard session management cannot be used, dedicated tokens are being used for this, which comply with the relevant Session Management security requirements. | N/A: see section note | - | - | N/A |
| V4.4.4 | 2 | Verify that dedicated WebSocket session management tokens are initially obtained or validated through the previously authenticated HTTPS session when transitioning an existing HTTPS session to a WebSocket channel. | N/A: see section note | - | - | N/A |

## V5.1 File Handling Documentation

**Planned control:** Document the app's files: no uploads; one download (the bake-off report CSV, S7 ADM-10 and ADM-12) with a server-fixed filename; the encrypted app folder file, whose name the server fixes

**Verification:** Review

| ID | L | Requirement | Control | Location | Verify | Status |
| --- | --- | --- | --- | --- | --- | --- |
| V5.1.1 | 2 | Verify that the documentation defines the permitted file types, expected file extensions, and maximum size (including unpacked size) for each upload feature. Additionally, ensure that the documentation specifies how files are made safe for end-users to download and process, such as how the application behaves when a malicious file is detected. | Files documented: one download (bake-off CSV), the app folder file, no uploads | docs/specs/S6, S7 | review | Planned |

## V5.2 File Upload and Content

**Not applicable:** No file upload. Attachments are never fetched or stored.

| ID | L | Requirement | Control | Location | Verify | Status |
| --- | --- | --- | --- | --- | --- | --- |
| V5.2.1 | 1 | Verify that the application will only accept files of a size which it can process without causing a loss of performance or a denial of service attack. | N/A: see section note | - | - | N/A |
| V5.2.2 | 1 | Verify that when the application accepts a file, either on its own or within an archive such as a zip file, it checks if the file extension matches an expected file extension and validates that the contents correspond to the type represented by the extension. This includes, but is not limited to, checking the initial 'magic bytes', performing image re-writing, and using specialized libraries for file content validation. For L1, this can focus just on files which are used to make specific business or security decisions. For L2 and up, this must apply to all files being accepted. | N/A: see section note | - | - | N/A |
| V5.2.3 | 2 | Verify that the application checks compressed files (e.g., zip, gz, docx, odt) against maximum allowed uncompressed size and against maximum number of files before uncompressing the file. | N/A: see section note | - | - | N/A |

## V5.3 File Storage

**Not applicable:** No public file storage. V5.3.2 applies to the app folder file (server-fixed name).

| ID | L | Requirement | Control | Location | Verify | Status |
| --- | --- | --- | --- | --- | --- | --- |
| V5.3.1 | 1 | Verify that files uploaded or generated by untrusted input and stored in a public folder, are not executed as server-side program code when accessed directly with an HTTP request. | N/A: see section note | - | - | N/A |
| V5.3.2 | 1 | Verify that when the application creates file paths for file operations, instead of user-submitted filenames, it uses internally generated or trusted data, or if user-submitted filenames or file metadata must be used, strict validation and sanitization must be applied. This is to protect against path traversal, local or remote file inclusion (LFI, RFI), and server-side request forgery (SSRF) attacks. | App folder file name fixed by the server; no user-supplied names | backend/appfolder | test `asvs_v5_3_2_*` | Planned |

## V5.4 File Download

**Applies in part:** the bake-off report CSV is a download, so V5.4.1 and V5.4.2 apply: the filename is fixed by the server, not built from query parameters, and RFC 6266 encoded. V5.4.3 is N/A: no file from an untrusted source is served.

| ID | L | Requirement | Control | Location | Verify | Status |
| --- | --- | --- | --- | --- | --- | --- |
| V5.4.1 | 2 | Verify that the application validates or ignores user-submitted filenames, including in a JSON, JSONP, or URL parameter and specifies a filename in the Content-Disposition header field in the response. | CSV filename fixed by the server, not built from query parameters; set in `Content-Disposition` | backend/admin | test `asvs_v5_4_1_*` | Planned |
| V5.4.2 | 2 | Verify that file names served (e.g., in HTTP response header fields or email attachments) are encoded or sanitized (e.g., following RFC 6266) to preserve document structure and prevent injection attacks. | Filename ASCII only, RFC 6266 encoded | backend/admin | test `asvs_v5_4_2_*` | Planned |
| V5.4.3 | 2 | Verify that files obtained from untrusted sources are scanned by antivirus scanners to prevent serving of known malicious content. | N/A: the only file served is the CSV built from aggregate data; no untrusted files | - | - | N/A |

## V6.1 Authentication Documentation

**Planned control:** Authentication design documented in S6 section 4: Google OAuth only for the trial (James, 3 October 2026: passkey lock dropped, moves to v2 pre-CASA hardening). V6.1.2 is N/A (no passwords)

**Verification:** Review

| ID | L | Requirement | Control | Location | Verify | Status |
| --- | --- | --- | --- | --- | --- | --- |
| V6.1.1 | 1 | Verify that application documentation defines how controls such as rate limiting, anti-automation, and adaptive response, are used to defend against attacks such as credential stuffing and password brute force. The documentation must make clear how these controls are configured and prevent malicious account lockout. | Rate limits on sign-in and invite redemption (S6 4, S7); no app-held credentials to lock out | docs/specs/S6 | review | Planned |
| V6.1.2 | 2 | Verify that a list of context-specific words is documented in order to prevent their use in passwords. The list could include permutations of organization names, product names, system identifiers, project codenames, department or role names, and similar. | N/A: no passwords | - | - | N/A |
| V6.1.3 | 2 | Verify that, if the application includes multiple authentication pathways, these are all documented together with the security controls and authentication strength which must be consistently enforced across them. | One pathway in S6 4: Google OAuth for first sign-in, sign-in, linking and step-up | docs/specs/S6 | review | Planned |

## V6.2 Password Security

**Not applicable:** No passwords exist anywhere in the system.

| ID | L | Requirement | Control | Location | Verify | Status |
| --- | --- | --- | --- | --- | --- | --- |
| V6.2.1 | 1 | Verify that user set passwords are at least 8 characters in length although a minimum of 15 characters is strongly recommended. | N/A: see section note | - | - | N/A |
| V6.2.2 | 1 | Verify that users can change their password. | N/A: see section note | - | - | N/A |
| V6.2.3 | 1 | Verify that password change functionality requires the user's current and new password. | N/A: see section note | - | - | N/A |
| V6.2.4 | 1 | Verify that passwords submitted during account registration or password change are checked against an available set of, at least, the top 3000 passwords which match the application's password policy, e.g. minimum length. | N/A: see section note | - | - | N/A |
| V6.2.5 | 1 | Verify that passwords of any composition can be used, without rules limiting the type of characters permitted. There must be no requirement for a minimum number of upper or lower case characters, numbers, or special characters. | N/A: see section note | - | - | N/A |
| V6.2.6 | 1 | Verify that password input fields use type=password to mask the entry. Applications may allow the user to temporarily view the entire masked password, or the last typed character of the password. | N/A: see section note | - | - | N/A |
| V6.2.7 | 1 | Verify that "paste" functionality, browser password helpers, and external password managers are permitted. | N/A: see section note | - | - | N/A |
| V6.2.8 | 1 | Verify that the application verifies the user's password exactly as received from the user, without any modifications such as truncation or case transformation. | N/A: see section note | - | - | N/A |
| V6.2.9 | 2 | Verify that passwords of at least 64 characters are permitted. | N/A: see section note | - | - | N/A |
| V6.2.10 | 2 | Verify that a user's password stays valid until it is discovered to be compromised or the user rotates it. The application must not require periodic credential rotation. | N/A: see section note | - | - | N/A |
| V6.2.11 | 2 | Verify that the documented list of context specific words is used to prevent easy to guess passwords being created. | N/A: see section note | - | - | N/A |
| V6.2.12 | 2 | Verify that passwords submitted during account registration or password changes are checked against a set of breached passwords. | N/A: see section note | - | - | N/A |

## V6.3 General Authentication Security

**Planned control:** Google OAuth sign-in, the one pathway; generic auth errors; anti-automation on sign-in and invite redemption. **V6.3.3 accepted deviation (trial only):** multi-factor relies on Google 2-Step Verification, with `amr` checked where Google provides it; accepted by James on 3 October 2026 for the trial; closed by the v2 passkey lock

**Verification:** Integration tests

| ID | L | Requirement | Control | Location | Verify | Status |
| --- | --- | --- | --- | --- | --- | --- |
| V6.3.1 | 1 | Verify that controls to prevent attacks such as credential stuffing and password brute force are implemented according to the application's security documentation. | Rate limits on sign-in and invite redemption; Google handles credential attacks | backend/auth | test `asvs_v6_3_1_*` | Planned |
| V6.3.2 | 1 | Verify that default user accounts (e.g., "root", "admin", or "sa") are not present in the application or are disabled. | No default accounts; first admin set by the `mt-admin` tool outside the API (T-507) | `backend/crates/admin-cli` | test | Planned |
| V6.3.3 | 2 | Verify that either a multi-factor authentication mechanism or a combination of single-factor authentication mechanisms, must be used in order to access the application. For L3, one of the factors must be a hardware-based authentication mechanism which provides compromise and impersonation resistance against phishing attacks while verifying the intent to authenticate by requiring a user-initiated action (such as a button press on a FIDO hardware key or a mobile phone). Relaxing any of the considerations in this requirement requires a fully documented rationale and a comprehensive set of mitigating controls. | Relies on Google 2-Step Verification; `amr` checked and logged where present. Closed by the v2 passkey lock | backend/auth | test `asvs_v6_3_3_*` | Accepted deviation (trial only) |
| V6.3.4 | 2 | Verify that, if the application includes multiple authentication pathways, there are no undocumented pathways and that security controls and authentication strength are enforced consistently. | One pathway (Google OAuth), same strength everywhere; no undocumented routes | backend/auth | test `asvs_v6_3_4_*` | Planned |

## V6.4 Authentication Factor Lifecycle and Recovery

**Planned control:** Single-use invite token for enrolment (S6 4). No app-held authentication factor and no in-app recovery flow in v1; Google account recovery applies, so V6.4.3 and V6.4.4 are N/A until the v2 passkey lock

**Verification:** Integration tests; review

| ID | L | Requirement | Control | Location | Verify | Status |
| --- | --- | --- | --- | --- | --- | --- |
| V6.4.1 | 1 | Verify that system generated initial passwords or activation codes are securely randomly generated, follow the existing password policy, and expire after a short period of time or after they are initially used. These initial secrets must not be permitted to become the long term password. | Invite token: 256-bit, single use, stored hashed, 7-day expiry; never a long-term credential | backend/auth | test `asvs_v6_4_1_*` | Planned |
| V6.4.2 | 1 | Verify that password hints or knowledge-based authentication (so-called "secret questions") are not present. | No hints or secret questions | app/ | review | Planned |
| V6.4.3 | 2 | Verify that a secure process for resetting a forgotten password is implemented, that does not bypass any enabled multi-factor authentication mechanisms. | N/A: no passwords and no in-app recovery flow; Google account recovery applies | - | - | N/A |
| V6.4.4 | 2 | Verify that if a multi-factor authentication factor is lost, evidence of identity proofing is performed at the same level as during enrollment. | N/A: no app-held factor in v1, so nothing to lose; Google account recovery applies. Revisit with the v2 passkey lock | - | - | N/A |

## V6.5 General Multi-factor authentication requirements

**Not applicable:** No lookup secrets, out-of-band codes or TOTP; sign-in is Google OAuth only.

**Verification:** Review

| ID | L | Requirement | Control | Location | Verify | Status |
| --- | --- | --- | --- | --- | --- | --- |
| V6.5.1 | 2 | Verify that lookup secrets, out-of-band authentication requests or codes, and time-based one-time passwords (TOTPs) are only successfully usable once. | N/A: no lookup secrets, out-of-band codes or TOTP | - | - | N/A |
| V6.5.2 | 2 | Verify that, when being stored in the application's backend, lookup secrets with less than 112 bits of entropy (19 random alphanumeric characters or 34 random digits) are hashed with an approved password storage hashing algorithm that incorporates a 32-bit random salt. A standard hash function can be used if the secret has 112 bits of entropy or more. | N/A: no lookup secrets | - | - | N/A |
| V6.5.3 | 2 | Verify that lookup secrets, out-of-band authentication code, and time-based one-time password seeds, are generated using a Cryptographically Secure Pseudorandom Number Generator (CSPRNG) to avoid predictable values. | N/A: no lookup secrets, out-of-band codes or TOTP seeds | - | - | N/A |
| V6.5.4 | 2 | Verify that lookup secrets and out-of-band authentication codes have a minimum of 20 bits of entropy (typically 4 random alphanumeric characters or 6 random digits is sufficient). | N/A: no lookup secrets or out-of-band codes | - | - | N/A |
| V6.5.5 | 2 | Verify that out-of-band authentication requests, codes, or tokens, as well as time-based one-time passwords (TOTPs) have a defined lifetime. Out of band requests must have a maximum lifetime of 10 minutes and for TOTP a maximum lifetime of 30 seconds. | N/A: no out-of-band requests or TOTP | - | - | N/A |

## V6.6 Out-of-Band authentication mechanisms

**Not applicable:** No out-of-band authenticators (SMS, email codes, push).

| ID | L | Requirement | Control | Location | Verify | Status |
| --- | --- | --- | --- | --- | --- | --- |
| V6.6.1 | 2 | Verify that authentication mechanisms using the Public Switched Telephone Network (PSTN) to deliver One-time Passwords (OTPs) via phone or SMS are offered only when the phone number has previously been validated, alternate stronger methods (such as Time based One-time Passwords) are also offered, and the service provides information on their security risks to users. For L3 applications, phone and SMS must not be available as options. | N/A: see section note | - | - | N/A |
| V6.6.2 | 2 | Verify that out-of-band authentication requests, codes, or tokens are bound to the original authentication request for which they were generated and are not usable for a previous or subsequent one. | N/A: see section note | - | - | N/A |
| V6.6.3 | 2 | Verify that a code based out-of-band authentication mechanism is protected against brute force attacks by using rate limiting. Consider also using a code with at least 64 bits of entropy. | N/A: see section note | - | - | N/A |

## V6.8 Authentication with an Identity Provider

**Planned control:** Google ID tokens validated (signature, issuer, audience, nonce, expiry); mailboxes keyed by provider plus subject, never email; invite token required with the email match. v2 Microsoft rule: key by `tid` plus `oid` and accept the email only with `xms_edov` true. V6.8.3 is N/A (no SAML)

**Verification:** Unit tests with forged tokens

| ID | L | Requirement | Control | Location | Verify | Status |
| --- | --- | --- | --- | --- | --- | --- |
| V6.8.1 | 2 | Verify that, if the application supports multiple identity providers (IdPs), the user's identity cannot be spoofed via another supported identity provider (eg. by using the same user identifier). The standard mitigation would be for the application to register and identify the user using a combination of the IdP ID (serving as a namespace) and the user's ID in the IdP. | Keyed by provider plus subject (Google `sub`; v2 Microsoft `tid` plus `oid`); v2 Microsoft email only with `xms_edov` true; invite token required | backend/auth | test `asvs_v6_8_1_*` | Planned |
| V6.8.2 | 2 | Verify that the presence and integrity of digital signatures on authentication assertions (for example on JWTs or SAML assertions) are always validated, rejecting any assertions that are unsigned or have invalid signatures. | ID token signature checked against provider keys; unsigned refused | backend/auth | test `asvs_v6_8_2_*` | Planned |
| V6.8.3 | 2 | Verify that SAML assertions are uniquely processed and used only once within the validity period to prevent replay attacks. | N/A: no SAML | - | - | N/A |
| V6.8.4 | 2 | Verify that, if an application uses a separate Identity Provider (IdP) and expects specific authentication strength, methods, or recentness for specific functions, the application verifies this using the information returned by the IdP. For example, if OIDC is used, this might be achieved by validating ID Token claims such as 'acr', 'amr', and 'auth_time' (if present). If the IdP does not provide this information, the application must have a documented fallback approach that assumes that the minimum strength authentication mechanism was used (for example, single-factor authentication using username and password). | Google `auth_time` checked for step-up (`max_age=300`); `amr` checked where present, else the V6.3.3 trial deviation applies (S6 4) | backend/auth | test `asvs_v6_8_4_*` | Planned |

## V7.1 Session Management Documentation

**Planned control:** Session design documented in S6

**Verification:** Review

| ID | L | Requirement | Control | Location | Verify | Status |
| --- | --- | --- | --- | --- | --- | --- |
| V7.1.1 | 2 | Verify that the user's session inactivity timeout and absolute maximum session lifetime are documented, are appropriate in combination with other controls, and that the documentation includes justification for any deviations from NIST SP 800-63B re-authentication requirements. | Timeouts and rationale in S6 4 | docs/specs/S6 | review | Planned |
| V7.1.2 | 2 | Verify that the documentation defines how many concurrent (parallel) sessions are allowed for one account as well as the intended behaviors and actions to be taken when the maximum number of active sessions is reached. | One session per user; a new sign-in ends the old one (S6 4) | docs/specs/S6 | review | Planned |
| V7.1.3 | 2 | Verify that all systems that create and manage user sessions as part of a federated identity management ecosystem (such as SSO systems) are documented along with controls to coordinate session lifetimes, termination, and any other conditions that require re-authentication. | No SSO: provider sessions neither create nor end ours (S6 4) | docs/specs/S6 | review | Planned |

## V7.2 Fundamental Session Management Security

**Planned control:** Opaque random session IDs, stored server side, rotated at sign-in

**Verification:** Integration tests

| ID | L | Requirement | Control | Location | Verify | Status |
| --- | --- | --- | --- | --- | --- | --- |
| V7.2.1 | 1 | Verify that the application performs all session token verification using a trusted, backend service. | Session looked up by hash in Firestore by `api` | backend/session | test `asvs_v7_2_1_*` | Planned |
| V7.2.2 | 1 | Verify that the application uses either self-contained or reference tokens that are dynamically generated for session management, i.e. not using static API secrets and keys. | Random reference token per session | backend/session | test `asvs_v7_2_2_*` | Planned |
| V7.2.3 | 1 | Verify that if reference tokens are used to represent user sessions, they are unique and generated using a cryptographically secure pseudo-random number generator (CSPRNG) and possess at least 128 bits of entropy. | 256-bit CSPRNG session IDs | backend/session | test `asvs_v7_2_3_*` | Planned |
| V7.2.4 | 1 | Verify that the application generates a new session token on user authentication, including re-authentication, and terminates the current session token. | New ID at sign-in and step-up; old ID dead | backend/session | test `asvs_v7_2_4_*` | Planned |

## V7.3 Session Timeout

**Planned control:** Idle 15 minutes and absolute 12 hours (decided by James, 3 October 2026)

**Verification:** Integration tests

| ID | L | Requirement | Control | Location | Verify | Status |
| --- | --- | --- | --- | --- | --- | --- |
| V7.3.1 | 2 | Verify that there is an inactivity timeout such that re-authentication is enforced according to risk analysis and documented security decisions. | Idle timeout 15 minutes (decided) | backend/session | test `asvs_v7_3_1_*` | Planned |
| V7.3.2 | 2 | Verify that there is an absolute maximum session lifetime such that re-authentication is enforced according to risk analysis and documented security decisions. | Absolute lifetime 12 hours (decided) | backend/session | test `asvs_v7_3_2_*` | Planned |

## V7.4 Session Termination

**Planned control:** One session per user; a new sign-in ends the old one; sign-out and account deletion end it; admin can end a user's session; disconnecting a mailbox revokes its tokens

**Verification:** Integration tests

| ID | L | Requirement | Control | Location | Verify | Status |
| --- | --- | --- | --- | --- | --- | --- |
| V7.4.1 | 1 | Verify that when session termination is triggered (such as logout or expiration), the application disallows any further use of the session. For reference tokens or stateful sessions, this means invalidating the session data at the application backend. Applications using self-contained tokens will need a solution such as maintaining a list of terminated tokens, disallowing tokens produced before a per-user date and time or rotating a per-user signing key. | Sign-out deletes the session record | backend/session | test `asvs_v7_4_1_*` | Planned |
| V7.4.2 | 1 | Verify that the application terminates all active sessions when a user account is disabled or deleted (such as an employee leaving the company). | Account deletion ends the user's session | backend/session | test `asvs_v7_4_2_*` | Planned |
| V7.4.3 | 2 | Verify that the application gives the option to terminate all other active sessions after a successful change or removal of any authentication factor (including password change via reset or recovery and, if present, an MFA settings update). | Met by one session per user; no app-held factor to change in v1 | backend/session | test `asvs_v7_4_3_*` | Planned |
| V7.4.4 | 2 | Verify that all pages that require authentication have easy and visible access to logout functionality. | Sign-out reachable from every signed-in screen | app/ | test `asvs_v7_4_4_*` | Planned |
| V7.4.5 | 2 | Verify that application administrators are able to terminate active sessions for an individual user or for all users. | Admin ends a user's session (deletes the one record); all users via the operations runbook (S11) | backend/admin | test `asvs_v7_4_5_*` | Planned |

## V7.5 Defenses Against Session Abuse

**Planned control:** Step-up by a fresh Google sign-in within 5 minutes (`prompt=login`, `max_age=300`, `auth_time` checked) before any account, mailbox or admin change (S6 4). One session per user, so no session list is needed for V7.5.2

**Verification:** Integration tests

| ID | L | Requirement | Control | Location | Verify | Status |
| --- | --- | --- | --- | --- | --- | --- |
| V7.5.1 | 2 | Verify that the application requires full re-authentication before allowing modifications to sensitive account attributes which may affect authentication such as email address, phone number, MFA configuration, or other information used in account recovery. | Step-up: fresh Google sign-in within 5 minutes (`prompt=login`, `max_age=300`, `auth_time` checked) for delete account, link or disconnect a mailbox, every admin write | backend/auth | test `asvs_v7_5_1_*` | Planned |
| V7.5.2 | 2 | Verify that users are able to view and (having authenticated again with at least one factor) terminate any or all currently active sessions. | Met by one session per user: the user sees and ends it with Sign out; a new sign-in ends the old one | backend/session, app/ | test `asvs_v7_5_2_*` | Planned |

## V7.6 Federated Re-authentication

**Planned control:** Provider re-consent handled; session ends when provider session revoked

**Verification:** Integration tests

| ID | L | Requirement | Control | Location | Verify | Status |
| --- | --- | --- | --- | --- | --- | --- |
| V7.6.1 | 2 | Verify that session lifetime and termination between Relying Parties (RPs) and Identity Providers (IdPs) behave as documented, requiring re-authentication as necessary such as when the maximum time between IdP authentication events is reached. | Step-up checks Google `auth_time`; revoked Google grant ends the job with Needs Attention "Sign in again" | backend/auth | test `asvs_v7_6_1_*` | Planned |
| V7.6.2 | 2 | Verify that creation of a session requires either the user's consent or an explicit action, preventing the creation of new application sessions without user interaction. | Sessions created only by a user-started Google sign-in | backend/auth | test `asvs_v7_6_2_*` | Planned |

## V8.1 Authorization Documentation

**Planned control:** Authorisation rules documented in S6: user owns mailboxes; admin role for invites

**Verification:** Review

| ID | L | Requirement | Control | Location | Verify | Status |
| --- | --- | --- | --- | --- | --- | --- |
| V8.1.1 | 1 | Verify that authorization documentation defines rules for restricting function-level and data-specific access based on consumer permissions and resource attributes. | Rules in S6 and the S7 route table: user owns mailboxes, jobs, sessions; admin role | docs/specs/S6, S7 | review | Planned |
| V8.1.2 | 2 | Verify that authorization documentation defines rules for field-level access restrictions (both read and write) based on consumer permissions and resource attributes. Note that these rules might depend on other attribute values of the relevant data object, such as state or status. | Field rules: admin sees no mail or token fields; typed DTOs per role | docs/specs/S7 | review | Planned |

## V8.2 General Authorization Design

**Planned control:** Every request checks the mailbox and job belong to the session user; deny by default

**Verification:** Multi-tenant isolation tests

| ID | L | Requirement | Control | Location | Verify | Status |
| --- | --- | --- | --- | --- | --- | --- |
| V8.2.1 | 1 | Verify that the application ensures that function-level access is restricted to consumers with explicit permissions. | Route auth level from the S7 table, deny by default | backend/http | test `asvs_v8_2_1_*` | Planned |
| V8.2.2 | 1 | Verify that the application ensures that data-specific access is restricted to consumers with explicit permissions to specific data items to mitigate insecure direct object reference (IDOR) and broken object level authorization (BOLA). | Ownership check on every ID (authorisation matrix) | backend/http | test `asvs_v8_2_2_*` | Planned |
| V8.2.3 | 2 | Verify that the application ensures that field-level access is restricted to consumers with explicit permissions to specific fields to mitigate broken object property level authorization (BOPLA). | Typed response and request DTOs per route | backend/http | test `asvs_v8_2_3_*` | Planned |

## V8.3 Operation Level Authorization

**Planned control:** Ownership re-checked in unsub and sweeper workers

**Verification:** Integration tests

| ID | L | Requirement | Control | Location | Verify | Status |
| --- | --- | --- | --- | --- | --- | --- |
| V8.3.1 | 1 | Verify that the application enforces authorization rules at a trusted service layer and doesn't rely on controls that an untrusted consumer could manipulate, such as client-side JavaScript. | Checks server side; workers re-check ownership | backend/http, backend/jobs | test `asvs_v8_3_1_*` | Planned |

## V8.4 Other Authorization Considerations

**Planned control:** Admin endpoints separated and logged

**Verification:** Integration tests

| ID | L | Requirement | Control | Location | Verify | Status |
| --- | --- | --- | --- | --- | --- | --- |
| V8.4.1 | 2 | Verify that multi-tenant applications use cross-tenant controls to ensure consumer operations will never affect tenants with which they do not have permissions to interact. | Cross-user isolation matrix generated from the route table | backend/http | test `asvs_v8_4_1_*` | Planned |

## V9.1 Token source and integrity

**Planned control:** We issue our own sealed tokens (feed cursors, undo tokens, classification tokens, prompt refs) under the one scheme in S6 section 5: AES-256-GCM under the user's `data_key`, associated data = token type, user ID, session record ID and expiry. Provider ID tokens verified against the provider's published keys

**Verification:** Unit tests

| ID | L | Requirement | Control | Location | Verify | Status |
| --- | --- | --- | --- | --- | --- | --- |
| V9.1.1 | 1 | Verify that self-contained tokens are validated using their digital signature or MAC to protect against tampering before accepting the token's contents. | Sealed tokens: GCM tag verified before use; provider ID tokens: signature verified | backend/crypto, backend/auth | test `asvs_v9_1_1_*` | Planned |
| V9.1.2 | 1 | Verify that only algorithms on an allowlist can be used to create and verify self-contained tokens, for a given context. The allowlist must include the permitted algorithms, ideally only either symmetric or asymmetric algorithms, and must not include the 'None' algorithm. If both symmetric and asymmetric must be supported, additional controls will be needed to prevent key confusion. | Algorithm fixed in code: AES-256-GCM for sealed tokens; provider-published set for ID tokens; `none` refused | backend/crypto, backend/auth | test `asvs_v9_1_2_*` | Planned |
| V9.1.3 | 1 | Verify that key material that is used to validate self-contained tokens is from trusted pre-configured sources for the token issuer, preventing attackers from specifying untrusted sources and keys. For JWTs and other JWS structures, headers such as 'jku', 'x5u', and 'jwk' must be validated against an allowlist of trusted sources. | Sealing key only from KMS-unwrapped `data_key`; ID token keys only from pinned provider key URLs; `jku`, `x5u`, `jwk` ignored | backend/crypto, backend/auth | test `asvs_v9_1_3_*` | Planned |

## V9.2 Token content

**Planned control:** Sealed tokens: type, user, session record and expiry bound as associated data, and the verifier uses the type it expects (V9.2.2). ID tokens: audience, issuer and expiry checked

**Verification:** Unit tests

| ID | L | Requirement | Control | Location | Verify | Status |
| --- | --- | --- | --- | --- | --- | --- |
| V9.2.1 | 1 | Verify that, if a validity time span is present in the token data, the token and its content are accepted only if the verification time is within this validity time span. For example, for JWTs, the claims 'nbf' and 'exp' must be verified. | Expiry in sealed-token associated data and checked; ID token `exp` and `nbf` checked | backend/crypto, backend/auth | test `asvs_v9_2_1_*` | Planned |
| V9.2.2 | 2 | Verify that the service receiving a token validates the token to be the correct type and is meant for the intended purpose before accepting the token's contents. For example, only access tokens can be accepted for authorization decisions and only ID Tokens can be used for proving user authentication. | Token type in associated data; verifier uses the type it expects (S6 5); ID tokens only for sign-in, linking and step-up | backend/crypto | test `asvs_v9_2_2_*` | Planned |
| V9.2.3 | 2 | Verify that the service only accepts tokens which are intended for use with that service (audience). For JWTs, this can be achieved by validating the 'aud' claim against an allowlist defined in the service. | Sealed tokens bound to user ID and session record ID; ID token `aud` equals our client ID | backend/crypto, backend/auth | test `asvs_v9_2_3_*` | Planned |
| V9.2.4 | 2 | Verify that, if a token issuer uses the same private key for issuing tokens to different audiences, the issued tokens contain an audience restriction that uniquely identifies the intended audiences. This will prevent a token from being reused with an unintended audience. If the audience identifier is dynamically provisioned, the token issuer must validate these audiences in order to make sure that they do not result in audience impersonation. | Per-user key plus user ID in associated data; one audience only | backend/crypto | review | Planned |

## V10.1 Generic OAuth and OIDC Security

**Planned control:** Tokens only in the backend (BFF); browser holds a session cookie only

**Verification:** Review; integration tests

| ID | L | Requirement | Control | Location | Verify | Status |
| --- | --- | --- | --- | --- | --- | --- |
| V10.1.1 | 2 | Verify that tokens are only sent to components that strictly need them. For example, when using a backend-for-frontend pattern for browser-based JavaScript applications, access and refresh tokens shall only be accessible for the backend. | Provider tokens stay in the backend; browser holds only the session cookie | backend/auth | test `asvs_v10_1_1_*` | Planned |
| V10.1.2 | 2 | Verify that the client only accepts values from the authorization server (such as the authorization code or ID Token) if these values result from an authorization flow that was initiated by the same user agent session and transaction. This requires that client-generated secrets, such as the proof key for code exchange (PKCE) 'code_verifier', 'state' or OIDC 'nonce', are not guessable, are specific to the transaction, and are securely bound to both the client and the user agent session in which the transaction was started. | PKCE verifier, `state` and `nonce` random and bound to the pre-auth session | backend/auth | test `asvs_v10_1_2_*` | Planned |

## V10.2 OAuth Client

**Planned control:** Authorisation code flow with PKCE, state and nonce; exact redirect URIs

**Verification:** Integration tests

| ID | L | Requirement | Control | Location | Verify | Status |
| --- | --- | --- | --- | --- | --- | --- |
| V10.2.1 | 2 | Verify that, if the code flow is used, the OAuth client has protection against browser-based request forgery attacks, commonly known as cross-site request forgery (CSRF), which trigger token requests, either by using proof key for code exchange (PKCE) functionality or checking the 'state' parameter that was sent in the authorization request. | PKCE plus `state` | backend/auth | test `asvs_v10_2_1_*` | Planned |
| V10.2.2 | 2 | Verify that, if the OAuth client can interact with more than one authorization server, it has a defense against mix-up attacks. For example, it could require that the authorization server return the 'iss' parameter value and validate it in the authorization response and the token response. | Per-provider `state`; `iss` checked against the expected provider | backend/auth | test `asvs_v10_2_2_*` | Planned |

## V10.3 OAuth Resource Server

**Not applicable:** The app API is not an OAuth resource server; it uses opaque session cookies.

| ID | L | Requirement | Control | Location | Verify | Status |
| --- | --- | --- | --- | --- | --- | --- |
| V10.3.1 | 2 | Verify that the resource server only accepts access tokens that are intended for use with that service (audience). The audience may be included in a structured access token (such as the 'aud' claim in JWT), or it can be checked using the token introspection endpoint. | N/A: see section note | - | - | N/A |
| V10.3.2 | 2 | Verify that the resource server enforces authorization decisions based on claims from the access token that define delegated authorization. If claims such as 'sub', 'scope', and 'authorization_details' are present, they must be part of the decision. | N/A: see section note | - | - | N/A |
| V10.3.3 | 2 | Verify that if an access control decision requires identifying a unique user from an access token (JWT or related token introspection response), the resource server identifies the user from claims that cannot be reassigned to other users. Typically, it means using a combination of 'iss' and 'sub' claims. | N/A: see section note | - | - | N/A |
| V10.3.4 | 2 | Verify that, if the resource server requires specific authentication strength, methods, or recentness, it verifies that the presented access token satisfies these constraints. For example, if present, using the OIDC 'acr', 'amr' and 'auth_time' claims respectively. | N/A: see section note | - | - | N/A |

## V10.4 OAuth Authorization Server

**Not applicable:** The app is not an OAuth authorisation server.

| ID | L | Requirement | Control | Location | Verify | Status |
| --- | --- | --- | --- | --- | --- | --- |
| V10.4.1 | 1 | Verify that the authorization server validates redirect URIs based on a client-specific allowlist of pre-registered URIs using exact string comparison. | N/A: see section note | - | - | N/A |
| V10.4.2 | 1 | Verify that, if the authorization server returns the authorization code in the authorization response, it can be used only once for a token request. For the second valid request with an authorization code that has already been used to issue an access token, the authorization server must reject a token request and revoke any issued tokens related to the authorization code. | N/A: see section note | - | - | N/A |
| V10.4.3 | 1 | Verify that the authorization code is short-lived. The maximum lifetime can be up to 10 minutes for L1 and L2 applications and up to 1 minute for L3 applications. | N/A: see section note | - | - | N/A |
| V10.4.4 | 1 | Verify that for a given client, the authorization server only allows the usage of grants that this client needs to use. Note that the grants 'token' (Implicit flow) and 'password' (Resource Owner Password Credentials flow) must no longer be used. | N/A: see section note | - | - | N/A |
| V10.4.5 | 1 | Verify that the authorization server mitigates refresh token replay attacks for public clients, preferably using sender-constrained refresh tokens, i.e., Demonstrating Proof of Possession (DPoP) or Certificate-Bound Access Tokens using mutual TLS (mTLS). For L1 and L2 applications, refresh token rotation may be used. If refresh token rotation is used, the authorization server must invalidate the refresh token after usage, and revoke all refresh tokens for that authorization if an already used and invalidated refresh token is provided. | N/A: see section note | - | - | N/A |
| V10.4.6 | 2 | Verify that, if the code grant is used, the authorization server mitigates authorization code interception attacks by requiring proof key for code exchange (PKCE). For authorization requests, the authorization server must require a valid 'code_challenge' value and must not accept a 'code_challenge_method' value of 'plain'. For a token request, it must require validation of the 'code_verifier' parameter. | N/A: see section note | - | - | N/A |
| V10.4.7 | 2 | Verify that if the authorization server supports unauthenticated dynamic client registration, it mitigates the risk of malicious client applications. It must validate client metadata such as any registered URIs, ensure the user's consent, and warn the user before processing an authorization request with an untrusted client application. | N/A: see section note | - | - | N/A |
| V10.4.8 | 2 | Verify that refresh tokens have an absolute expiration, including if sliding refresh token expiration is applied. | N/A: see section note | - | - | N/A |
| V10.4.9 | 2 | Verify that refresh tokens and reference access tokens can be revoked by an authorized user using the authorization server user interface, to mitigate the risk of malicious clients or stolen tokens. | N/A: see section note | - | - | N/A |
| V10.4.10 | 2 | Verify that confidential client is authenticated for client-to-authorized server backchannel requests such as token requests, pushed authorization requests (PAR), and token revocation requests. | N/A: see section note | - | - | N/A |
| V10.4.11 | 2 | Verify that the authorization server configuration only assigns the required scopes to the OAuth client. | N/A: see section note | - | - | N/A |

## V10.5 OIDC Client

**Planned control:** OIDC client validation of ID tokens and nonce

**Verification:** Unit tests

| ID | L | Requirement | Control | Location | Verify | Status |
| --- | --- | --- | --- | --- | --- | --- |
| V10.5.1 | 2 | Verify that the client (as the relying party) mitigates ID Token replay attacks. For example, by ensuring that the 'nonce' claim in the ID Token matches the 'nonce' value sent in the authentication request to the OpenID Provider (in OAuth2 refereed to as the authorization request sent to the authorization server). | `nonce` matched and single use | backend/auth | test `asvs_v10_5_1_*` | Planned |
| V10.5.2 | 2 | Verify that the client uniquely identifies the user from ID Token claims, usually the 'sub' claim, which cannot be reassigned to other users (for the scope of an identity provider). | User keyed by Google `sub`; v2 Microsoft `tid` plus `oid` | backend/auth | test `asvs_v10_5_2_*` | Planned |
| V10.5.3 | 2 | Verify that the client rejects attempts by a malicious authorization server to impersonate another authorization server through authorization server metadata. The client must reject authorization server metadata if the issuer URL in the authorization server metadata does not exactly match the pre-configured issuer URL expected by the client. | Issuer pinned per provider; metadata issuer must match exactly | backend/auth | test `asvs_v10_5_3_*` | Planned |
| V10.5.4 | 2 | Verify that the client validates that the ID Token is intended to be used for that client (audience) by checking that the 'aud' claim from the token is equal to the 'client_id' value for the client. | `aud` equals our client ID | backend/auth | test `asvs_v10_5_4_*` | Planned |
| V10.5.5 | 2 | Verify that, when using OIDC back-channel logout, the relying party mitigates denial of service through forced logout and cross-JWT confusion in the logout flow. The client must verify that the logout token is correctly typed with a value of 'logout+jwt', contains the 'event' claim with the correct member name, and does not contain a 'nonce' claim. Note that it is also recommended to have a short expiration (e.g., 2 minutes). | N/A: no OIDC back-channel logout | - | - | N/A |

## V10.6 OpenID Provider

**Not applicable:** The app is not an OpenID provider.

| ID | L | Requirement | Control | Location | Verify | Status |
| --- | --- | --- | --- | --- | --- | --- |
| V10.6.1 | 2 | Verify that the OpenID Provider only allows values 'code', 'ciba', 'id_token', or 'id_token code' for response mode. Note that 'code' is preferred over 'id_token code' (the OIDC Hybrid flow), and 'token' (any Implicit flow) must not be used. | N/A: see section note | - | - | N/A |
| V10.6.2 | 2 | Verify that the OpenID Provider mitigates denial of service through forced logout. By obtaining explicit confirmation from the end-user or, if present, validating parameters in the logout request (initiated by the relying party), such as the 'id_token_hint'. | N/A: see section note | - | - | N/A |

## V10.7 Consent Management

**Planned control:** Minimal scopes per feature; consent screen lists them; scopes re-checked on token use

**Verification:** Review; integration tests

| ID | L | Requirement | Control | Location | Verify | Status |
| --- | --- | --- | --- | --- | --- | --- |
| V10.7.1 | 2 | Verify that the authorization server ensures that the user consents to each authorization request. If the identity of the client cannot be assured, the authorization server must always explicitly prompt the user for consent. | Provider consent screens; minimal scopes requested | backend/auth | review | Planned |
| V10.7.2 | 2 | Verify that when the authorization server prompts for user consent, it presents sufficient and clear information about what is being consented to. When applicable, this should include the nature of the requested authorizations (typically based on scope, resource server, Rich Authorization Requests (RAR) authorization details), the identity of the authorized application, and the lifetime of these authorizations. | Scopes explained on our screen before redirect | app/ | review | Planned |
| V10.7.3 | 2 | Verify that the user can review, modify, and revoke consents which the user has granted through the authorization server. | Settings lists linked mailboxes; disconnect revokes the grant | backend/auth, app/ | test `asvs_v10_7_3_*` | Planned |

## V11.1 Cryptographic Inventory and Documentation

**Planned control:** Cryptographic inventory in S6 section 5: two KMS keys (`data-key-kek`, `system-fields`), one per-user `data_key` (refresh tokens, app folder, sealed tokens, encrypted fields), email lookup and log pseudonymisation HMAC keys, session IDs, invite tokens, sealed tokens

**Verification:** Review

| ID | L | Requirement | Control | Location | Verify | Status |
| --- | --- | --- | --- | --- | --- | --- |
| V11.1.1 | 2 | Verify that there is a documented policy for management of cryptographic keys and a cryptographic key lifecycle that follows a key management standard such as NIST SP 800-57. This should include ensuring that keys are not overshared (for example, with more than two entities for shared secrets and more than one entity for private keys). | Key lifecycle in S6 5 (generate, wrap, rotate, destroy); KMS use limited to three service accounts | docs/specs/S6 | review | Planned |
| V11.1.2 | 2 | Verify that a cryptographic inventory is performed, maintained, regularly updated, and includes all cryptographic keys, algorithms, and certificates used by the application. It must also document where keys can and cannot be used in the system, and the types of data that can and cannot be protected using the keys. | Inventory in S6 5 lists every key with allowed and forbidden uses | docs/specs/S6 | review | Planned |

## V11.2 Secure Cryptography Implementation

**Planned control:** Vetted libraries only (Cloud KMS, RustCrypto aes-gcm or ring); no custom crypto

**Verification:** Review; cargo-deny

| ID | L | Requirement | Control | Location | Verify | Status |
| --- | --- | --- | --- | --- | --- | --- |
| V11.2.1 | 2 | Verify that industry-validated implementations (including libraries and hardware-accelerated implementations) are used for cryptographic operations. | Vetted crates and Cloud KMS only; cargo-deny allowlist | backend/crypto | ci | Planned |
| V11.2.2 | 2 | Verify that the application is designed with crypto agility such that random number, authenticated encryption, MAC, or hashing algorithms, key lengths, rounds, ciphers and modes can be reconfigured, upgraded, or swapped at any time, to protect against cryptographic breaks. Similarly, it must also be possible to replace keys and passwords and re-encrypt data. This will allow for seamless upgrades to post-quantum cryptography (PQC), once high-assurance implementations of approved PQC schemes or standards are widely available. | Scheme version stored with each ciphertext and sealed token; re-encrypt path | backend/crypto | test `asvs_v11_2_2_*` | Tested |
| V11.2.3 | 2 | Verify that all cryptographic primitives utilize a minimum of 128-bits of security based on the algorithm, key size, and configuration. For example, a 256-bit ECC key provides roughly 128 bits of security where RSA requires a 3072-bit key to achieve 128 bits of security. | AES-256, SHA-256 and P-256 or stronger | backend/crypto | review | Planned |

## V11.3 Encryption Algorithms

**Planned control:** AES-256-GCM with associated data (user ID and field name)

**Verification:** Unit tests

| ID | L | Requirement | Control | Location | Verify | Status |
| --- | --- | --- | --- | --- | --- | --- |
| V11.3.1 | 1 | Verify that insecure block modes (e.g., ECB) and weak padding schemes (e.g., PKCS#1 v1.5) are not used. | AES-GCM only; no ECB or PKCS#1 v1.5 | backend/crypto | mailtinder-crypto-approved-aead-only | Tested |
| V11.3.2 | 1 | Verify that only approved ciphers and modes such as AES with GCM are used. | AES-256-GCM only | backend/crypto | mailtinder-crypto-approved-aead-only | Tested |
| V11.3.3 | 2 | Verify that encrypted data is protected against unauthorized modification preferably by using an approved authenticated encryption method or by combining an approved encryption method with an approved MAC algorithm. | AEAD with associated data binding (user, field or token type) | backend/crypto | test `asvs_v11_3_3_*` | Tested |

## V11.4 Hashing and Hash-based Functions

**Planned control:** SHA-256 or better; keyed HMAC for email lookup hashes

**Verification:** Unit tests

| ID | L | Requirement | Control | Location | Verify | Status |
| --- | --- | --- | --- | --- | --- | --- |
| V11.4.1 | 1 | Verify that only approved hash functions are used for general cryptographic use cases, including digital signatures, HMAC, KDF, and random bit generation. Disallowed hash functions, such as MD5, must not be used for any cryptographic purpose. | SHA-256, HMAC-SHA-256, HKDF-SHA-256; MD5 and SHA-1 banned | backend/ | mailtinder-crypto-no-weak-hash | Tested |
| V11.4.2 | 2 | Verify that passwords are stored using an approved, computationally intensive, key derivation function (also known as a "password hashing function"), with parameter settings configured based on current guidance. The settings should balance security and performance to make brute-force attacks sufficiently challenging for the required level of security. | N/A: no passwords stored | - | - | N/A |
| V11.4.3 | 2 | Verify that hash functions used in digital signatures, as part of data authentication or data integrity are collision resistant and have appropriate bit-lengths. If collision resistance is required, the output length must be at least 256 bits. If only resistance to second pre-image attacks is required, the output length must be at least 128 bits. | SHA-256 for every integrity use | backend/crypto | review | Planned |
| V11.4.4 | 2 | Verify that the application uses approved key derivation functions with key stretching parameters when deriving secret keys from passwords. The parameters in use must balance security and performance to prevent brute-force attacks from compromising the resulting cryptographic key. | N/A: no keys derived from passwords; HKDF inputs are high entropy | - | - | N/A |

## V11.5 Random Values

**Planned control:** OS CSPRNG for IDs, nonces and keys

**Verification:** Code review

| ID | L | Requirement | Control | Location | Verify | Status |
| --- | --- | --- | --- | --- | --- | --- |
| V11.5.1 | 2 | Verify that all random numbers and strings which are intended to be non-guessable must be generated using a cryptographically secure pseudo-random number generator (CSPRNG) and have at least 128 bits of entropy. Note that UUIDs do not respect this condition. | OS CSPRNG for IDs, tokens, nonces and keys; UUIDs never used as secrets | backend/ | mailtinder-no-non-csprng-secrets | Tested |

## V11.6 Public Key Cryptography

**Planned control:** Provider ID token signature verification with vetted libraries

**Verification:** Unit tests

| ID | L | Requirement | Control | Location | Verify | Status |
| --- | --- | --- | --- | --- | --- | --- |
| V11.6.1 | 2 | Verify that only approved cryptographic algorithms and modes of operation are used for key generation and seeding, and digital signature generation and verification. Key generation algorithms must not generate insecure keys vulnerable to known attacks, for example, RSA keys which are vulnerable to Fermat factorization. | ID token signatures verified with vetted libraries; no key pairs generated | backend/auth | test `asvs_v11_6_1_*` | Planned |

## V12.1 General TLS Security Guidance

**Planned control:** TLS 1.2 minimum, 1.3 preferred, managed certificates on Firebase Hosting and Cloud Run

**Verification:** Configuration test

| ID | L | Requirement | Control | Location | Verify | Status |
| --- | --- | --- | --- | --- | --- | --- |
| V12.1.1 | 1 | Verify that only the latest recommended versions of the TLS protocol are enabled, such as TLS 1.2 and TLS 1.3. The latest version of the TLS protocol must be the preferred option. | TLS 1.2 and 1.3 at the Google front end | Firebase Hosting, Cloud Run | dast | Planned |
| V12.1.2 | 2 | Verify that only recommended cipher suites are enabled, with the strongest cipher suites set as preferred. L3 applications must only support cipher suites which provide forward secrecy. | Google-managed cipher suites | Firebase Hosting, Cloud Run | review | Planned |
| V12.1.3 | 2 | Verify that the application validates that mTLS client certificates are trusted before using the certificate identity for authentication or authorization. | N/A: no mTLS client certificates | - | - | N/A |

## V12.2 HTTPS Communication with External Facing Services

**Planned control:** HTTPS only for all external calls; plain http unsubscribe links refused. In v1 the only mail-supplied destination is the `unsub` one-click POST (SSRF checks in V1.3.6, no redirects in V15.3.2); the page handler is v2 scope

**Verification:** Unit tests

| ID | L | Requirement | Control | Location | Verify | Status |
| --- | --- | --- | --- | --- | --- | --- |
| V12.2.1 | 1 | Verify that TLS is used for all connectivity between a client and external facing, HTTP-based services, and does not fall back to insecure or unencrypted communications. | HTTPS only for every call; `http` unsubscribe links refused | backend/egress | test `asvs_v12_2_1_*` | Planned |
| V12.2.2 | 1 | Verify that external facing services use publicly trusted TLS certificates. | Google-managed public certificates | Firebase Hosting, Cloud Run | review | Planned |

## V12.3 General Service to Service Communication Security

**Planned control:** Service to service over HTTPS with Google-signed OIDC identity tokens

**Verification:** Integration tests

| ID | L | Requirement | Control | Location | Verify | Status |
| --- | --- | --- | --- | --- | --- | --- |
| V12.3.1 | 2 | Verify that an encrypted protocol such as TLS is used for all inbound and outbound connections to and from the application, including monitoring systems, management tools, remote access and SSH, middleware, databases, mainframes, partner systems, or external APIs. The server must not fall back to insecure or unencrypted protocols. | All outbound connections over TLS (rustls) | backend/egress | test `asvs_v12_3_1_*` | Planned |
| V12.3.2 | 2 | Verify that TLS clients validate certificates received before communicating with a TLS server. | Certificate verification never disabled | backend/egress | semgrep | Planned |
| V12.3.3 | 2 | Verify that TLS or another appropriate transport encryption mechanism used for all connectivity between internal, HTTP-based services within the application, and does not fall back to insecure or unencrypted communications. | Service to service over HTTPS with OIDC tokens | Terraform | review | Planned |
| V12.3.4 | 2 | Verify that TLS connections between internal services use trusted certificates. Where internally generated or self-signed certificates are used, the consuming service must be configured to only trust specific internal CAs and specific self-signed certificates. | Google-managed certificates on `run.app` | Cloud Run | review | Planned |

## V13.1 Configuration Documentation

**Planned control:** Configuration documented in Terraform and S4

**Verification:** Review

| ID | L | Requirement | Control | Location | Verify | Status |
| --- | --- | --- | --- | --- | --- | --- |
| V13.1.1 | 2 | Verify that all communication needs for the application are documented. This must include external services which the application relies upon and cases where an end user might be able to provide an external location to which the application will then connect. | Communication needs in S4 and the `HttpEgress` allowlist; full document in S11 | docs/specs/S4 | review | Planned |

## V13.2 Backend Communication Configuration

**Planned control:** Least-privilege service accounts per service; internal ingress for workers; per-service egress host allowlist enforced at the `HttpEgress` port, with a test (S4 5.7). Cloud Run has no domain egress filter, so the port is the control

**Verification:** Terraform review

| ID | L | Requirement | Control | Location | Verify | Status |
| --- | --- | --- | --- | --- | --- | --- |
| V13.2.1 | 2 | Verify that communications between backend application components that don't support the application's standard user session mechanism, including APIs, middleware, and data layers, are authenticated. Authentication must use individual service accounts, short-term tokens, or certificate-based authentication and not unchanging credentials such as passwords, API keys, or shared accounts with privileged access. | Per-service accounts; OIDC tokens between services; no shared keys | Terraform | review | Planned |
| V13.2.2 | 2 | Verify that communications between backend application components, including local or operating system services, APIs, middleware, and data layers, are performed with accounts assigned the least necessary privileges. | Least-privilege IAM (S4 2): KMS for `api`, `unsub`, `worker` only; `api` also Vertex AI user and Secret Manager on Jev key, HMAC keys, OAuth secrets | Terraform | review | Planned |
| V13.2.3 | 2 | Verify that if a credential has to be used for service authentication, the credential being used by the consumer is not a default credential (e.g., root/root or admin/admin). | No default credentials; Google identities only | Terraform | review | Planned |
| V13.2.4 | 2 | Verify that an allowlist is used to define the external resources or systems with which the application is permitted to communicate (e.g., for outbound requests, data loads, or file access). This allowlist can be implemented at the application layer, web server, firewall, or a combination of different layers. | Per-service host allowlist at the `HttpEgress` port: `api` Gmail, Drive, Google OAuth, Vertex AI, `api.typesafe.ai`; `unsub` Gmail send, the Google OAuth token and certificate endpoints, validated one-click target (no Drive); `worker` the Google OAuth certificate endpoint. Google Cloud APIs and the metadata token endpoint go through the platform client, allowlisted by host plus path. v2: Graph, OneDrive, Microsoft OAuth, `pagehandler` | backend/egress | test `asvs_v13_2_4_*` | Planned |
| V13.2.5 | 2 | Verify that the web or application server is configured with an allowlist of resources or systems to which the server can send requests or load data or files from. | Same allowlist; any other host refused before connecting | backend/egress | test `asvs_v13_2_5_*` | Planned |

## V13.3 Secret Management

**Planned control:** Secrets in Secret Manager; Gitleaks in pre-commit and CI

**Verification:** CI check

| ID | L | Requirement | Control | Location | Verify | Status |
| --- | --- | --- | --- | --- | --- | --- |
| V13.3.1 | 2 | Verify that a secrets management solution, such as a key vault, is used to securely create, store, control access to, and destroy backend secrets. These could include passwords, key material, integrations with databases and third-party systems, keys and seeds for time-based tokens, other internal secrets, and API keys. Secrets must not be included in application source code or included in build artifacts. For an L3 application, this must involve a hardware-backed solution such as an HSM. | Secret Manager; Gitleaks in pre-commit and CI | Terraform, CI | ci | Planned |
| V13.3.2 | 2 | Verify that access to secret assets adheres to the principle of least privilege. | One accessor per secret (S4 2) | Terraform | review | Planned |

## V13.4 Unintended Information Leakage

**Planned control:** No debug endpoints, stack traces or version banners in production

**Verification:** Integration tests

| ID | L | Requirement | Control | Location | Verify | Status |
| --- | --- | --- | --- | --- | --- | --- |
| V13.4.1 | 1 | Verify that the application is deployed either without any source control metadata, including the .git or .svn folders, or in a way that these folders are inaccessible both externally and to the application itself. | Container holds only the release binary | backend/Dockerfile | test `asvs_v13_4_1_*` | Planned |
| V13.4.2 | 2 | Verify that debug modes are disabled for all components in production environments to prevent exposure of debugging features and information leakage. | Release builds have no debug features | backend/ | test `asvs_v13_4_2_*` | Planned |
| V13.4.3 | 2 | Verify that web servers do not expose directory listings to clients unless explicitly intended. | No directory listing on Hosting or `api` | firebase.json | dast | Planned |
| V13.4.4 | 2 | Verify that using the HTTP TRACE method is not supported in production environments, to avoid potential information leakage. | `TRACE` refused | backend/http | test `asvs_v13_4_4_*` | Planned |
| V13.4.5 | 2 | Verify that documentation (such as for internal APIs) and monitoring endpoints are not exposed unless explicitly intended. | No docs or metrics routes on `api` | backend/http | test `asvs_v13_4_5_*` | Planned |

## V14.1 Data Protection Documentation

**Planned control:** Data inventory and classification in S5

**Verification:** Review

| ID | L | Requirement | Control | Location | Verify | Status |
| --- | --- | --- | --- | --- | --- | --- |
| V14.1.1 | 2 | Verify that all sensitive data created and processed by the application has been identified and classified into protection levels. This includes data that is only encoded and therefore easily decoded, such as Base64 strings or the plaintext payload inside a JWT. Protection levels need to take into account any data protection and privacy regulations and standards which the application is required to comply with. | Data inventory and classes in S5 | docs/specs/S5 | review | Planned |
| V14.1.2 | 2 | Verify that all sensitive data protection levels have a documented set of protection requirements. This must include (but not be limited to) requirements related to general encryption, integrity verification, retention, how the data is to be logged, access controls around sensitive data in logs, database-level encryption, privacy and privacy-enhancing technologies to be used, and other confidentiality requirements. | Protection rules per class in S5 | docs/specs/S5 | review | Planned |

## V14.2 General Data Protection

**Planned control:** No mail content at rest; no sensitive data in URLs, logs or caches; Cache-Control no-store on mail responses

**Verification:** Log-scanning test; privacy Semgrep rules

| ID | L | Requirement | Control | Location | Verify | Status |
| --- | --- | --- | --- | --- | --- | --- |
| V14.2.1 | 1 | Verify that sensitive data is only sent to the server in the HTTP message body or header fields, and that the URL and query string do not contain sensitive information, such as an API key or session token. | No sensitive data in URLs (OpenAPI path lint) | docs/specs/S7 | test `asvs_v14_2_1_*` | Planned |
| V14.2.2 | 2 | Verify that the application prevents sensitive data from being cached in server components, such as load balancers and application caches, or ensures that the data is securely purged after use. | `Cache-Control: no-store`; no server caches of mail | backend/http | test `asvs_v14_2_2_*` | Planned |
| V14.2.3 | 2 | Verify that defined sensitive data is not sent to untrusted parties (e.g., user trackers) to prevent unwanted collection of data outside of the application's control. | No trackers; model calls only with consent | app/, backend/feed | test `asvs_v14_2_3_*` | Planned |
| V14.2.4 | 2 | Verify that controls around sensitive data related to encryption, integrity verification, retention, how the data is to be logged, access controls around sensitive data in logs, privacy and privacy-enhancing technologies, are implemented as defined in the documentation for the specific data's protection level. | S5 controls per class; leak tests (XC-01) | backend/ | test `asvs_v14_2_4_*` | Planned |

## V14.3 Client-side Data Protection

**Planned control:** Nothing written to localStorage, sessionStorage or IndexedDB; cleared on sign-out

**Verification:** Browser tests

| ID | L | Requirement | Control | Location | Verify | Status |
| --- | --- | --- | --- | --- | --- | --- |
| V14.3.1 | 1 | Verify that authenticated data is cleared from client storage, such as the browser DOM, after the client or session is terminated. The 'Clear-Site-Data' HTTP response header field may be able to help with this but the client-side should also be able to clear up if the server connection is not available when the session is terminated. | `Clear-Site-Data` on sign-out; app wipes memory on any `401` | backend/session, app/ | test `asvs_v14_3_1_*` | Planned |
| V14.3.2 | 2 | Verify that the application sets sufficient anti-caching HTTP response header fields (i.e., Cache-Control: no-store) so that sensitive data is not cached in browsers. | `Cache-Control: no-store` on API responses | backend/http | test `asvs_v14_3_2_*` | Planned |
| V14.3.3 | 2 | Verify that data stored in browser storage (such as localStorage, sessionStorage, IndexedDB, or cookies) does not contain sensitive data, with the exception of session tokens. | Nothing in browser storage except the session cookie | app/ | test `asvs_v14_3_3_*` | Planned |

## V15.1 Secure Coding and Architecture Documentation

**Planned control:** Threat model and architecture in S4 and S6; dependency policy

**Verification:** Review

| ID | L | Requirement | Control | Location | Verify | Status |
| --- | --- | --- | --- | --- | --- | --- |
| V15.1.1 | 1 | Verify that application documentation defines risk based remediation time frames for 3rd party component versions with vulnerabilities and for updating libraries in general, to minimize the risk from these components. | Remediation windows in S11 (not yet written) | docs/specs/S11 | review | Planned |
| V15.1.2 | 2 | Verify that an inventory catalog, such as software bill of materials (SBOM), is maintained of all third-party libraries in use, including verifying that components come from pre-defined, trusted, and continually maintained repositories. | SBOM from lock files; cargo-deny source allowlist | CI | ci | Planned |
| V15.1.3 | 2 | Verify that the application documentation identifies functionality which is time-consuming or resource-demanding. This must include how to prevent a loss of availability due to overusing this functionality and how to avoid a situation where building a response takes longer than the consumer's timeout. Potential defenses may include asynchronous processing, using queues, and limiting parallel processes per user and per application. | Heavy paths (feed build, model calls; page handler in v2) and their limits in S4 and S7 | docs/specs/S4, S7 | review | Planned |

## V15.2 Security Architecture and Dependencies

**Planned control:** cargo-audit, cargo-deny, Dependabot, Artifact Registry scanning; page handler isolated (v2 scope)

**Verification:** CI checks

| ID | L | Requirement | Control | Location | Verify | Status |
| --- | --- | --- | --- | --- | --- | --- |
| V15.2.1 | 1 | Verify that the application only contains components which have not breached the documented update and remediation time frames. | cargo-audit, Dependabot, Dart licence check | CI | ci | Planned |
| V15.2.2 | 2 | Verify that the application has implemented defenses against loss of availability due to functionality which is time-consuming or resource-demanding, based on the documented security decisions and strategies for this. | Timeouts, concurrency caps, Cloud Tasks queues, 2-second model timeout | backend/ | test `asvs_v15_2_2_*` | Planned |
| V15.2.3 | 2 | Verify that the production environment only includes functionality that is required for the application to function, and does not expose extraneous functionality such as test code, sample snippets, and development functionality. | No test routes or dev features in release builds | backend/ | test `asvs_v15_2_3_*` | Planned |

## V15.3 Defensive Coding

**Planned control:** Defensive coding rules: no panics on external input, typed errors, mass-assignment protection via typed DTOs

**Verification:** Clippy; unit tests

| ID | L | Requirement | Control | Location | Verify | Status |
| --- | --- | --- | --- | --- | --- | --- |
| V15.3.1 | 1 | Verify that the application only returns the required subset of fields from a data object. For example, it should not return an entire data object, as some individual fields should not be accessible to users. | Typed response DTOs | backend/http | test `asvs_v15_3_1_*` | Planned |
| V15.3.2 | 2 | Verify that where the application backend makes calls to external URLs, it is configured to not follow redirects unless it is intended functionality. | Redirects off in `HttpEgress`; a 3xx on the one-click POST raises Needs Attention, no retry. Page handler per-hop checks are v2 scope | backend/egress | test `asvs_v15_3_2_*` | Planned |
| V15.3.3 | 2 | Verify that the application has countermeasures to protect against mass assignment attacks by limiting allowed fields per controller and action, e.g., it is not possible to insert or update a field value when it was not intended to be part of that action. | Typed request DTOs with `deny_unknown_fields` | backend/http | test `asvs_v15_3_3_*` | Planned |
| V15.3.4 | 2 | Verify that all proxying and middleware components transfer the user's original IP address correctly using trusted data fields that cannot be manipulated by the end user, and the application and web server use this correct value for logging and security decisions such as rate limiting, taking into account that even the original IP address may not be reliable due to dynamic IPs, VPNs, or corporate firewalls. | Client IP from the trusted front-end position only | backend/http | test `asvs_v15_3_4_*` | Planned |
| V15.3.5 | 2 | Verify that the application explicitly ensures that variables are of the correct type and performs strict equality and comparator operations. This is to avoid type juggling or type confusion vulnerabilities caused by the application code making an assumption about a variable type. | Rust static typing; no implicit conversion | backend/ | ci | Planned |
| V15.3.6 | 2 | Verify that JavaScript code is written in a way that prevents prototype pollution, for example, by using Set() or Map() instead of object literals. | No handwritten JavaScript; Dart only | app/ | review | Planned |
| V15.3.7 | 2 | Verify that the application has defenses against HTTP parameter pollution attacks, particularly if the application framework makes no distinction about the source of request parameters (query string, body parameters, cookies, or header fields). | Each parameter read from one declared source | backend/http | test `asvs_v15_3_7_*` | Planned |

## V16.1 Security Logging Documentation

**Planned control:** Logging inventory in S6: allowlisted fields only

**Verification:** Review

| ID | L | Requirement | Control | Location | Verify | Status |
| --- | --- | --- | --- | --- | --- | --- |
| V16.1.1 | 2 | Verify that an inventory exists documenting the logging performed at each layer of the application's technology stack, what events are being logged, log formats, where that logging is stored, how it is used, how access to it is controlled, and for how long logs are kept. | Logging inventory in S6 7 and S5 | docs/specs/S6, S5 | review | Planned |

## V16.2 General Logging

**Planned control:** Structured logs with request ID, pseudonymous user ID, route, outcome

**Verification:** Log-scanning test

| ID | L | Requirement | Control | Location | Verify | Status |
| --- | --- | --- | --- | --- | --- | --- |
| V16.2.1 | 2 | Verify that each log entry includes necessary metadata (such as when, where, who, what) that would allow for a detailed investigation of the timeline when an event happens. | Structured fields: time, request ID, pseudonymous user, route, outcome | backend/logging | test `asvs_v16_2_1_*` | Planned |
| V16.2.2 | 2 | Verify that time sources for all logging components are synchronized, and that timestamps in security event metadata use UTC or include an explicit time zone offset. UTC is recommended to ensure consistency across distributed systems and to prevent confusion during daylight saving time transitions. | Cloud Logging UTC timestamps | backend/logging | review | Planned |
| V16.2.3 | 2 | Verify that the application only stores or broadcasts logs to the files and services that are documented in the log inventory. | Logs go only to the Cloud Logging bucket | Terraform | review | Planned |
| V16.2.4 | 2 | Verify that logs can be read and correlated by the log processor that is in use, preferably by using a common logging format. | JSON structured logs | backend/logging | test `asvs_v16_2_4_*` | Planned |
| V16.2.5 | 2 | Verify that when logging sensitive data, the application enforces logging based on the data's protection level. For example, it may not be allowed to log certain data, such as credentials or payment details. Other data, such as session tokens, may only be logged by being hashed or masked, either in full or partially. | Allowlisted fields; IDs pseudonymised with the log HMAC key; no tokens | backend/logging | test `asvs_v16_2_5_*` | Planned |

## V16.3 Security Events

**Planned control:** Security events logged: sign-in, invite, admin actions, rate-limit hits, authorisation failures

**Verification:** Integration tests

| ID | L | Requirement | Control | Location | Verify | Status |
| --- | --- | --- | --- | --- | --- | --- |
| V16.3.1 | 2 | Verify that all authentication operations are logged, including successful and unsuccessful attempts. Additional metadata, such as the type of authentication or factors used, should also be collected. | Every sign-in and step-up attempt logged, with `amr` where present | backend/auth | test `asvs_v16_3_1_*` | Planned |
| V16.3.2 | 2 | Verify that failed authorization attempts are logged. For L3, this must include logging all authorization decisions, including logging when sensitive data is accessed (without logging the sensitive data itself). | Authorisation failures logged | backend/http | test `asvs_v16_3_2_*` | Planned |
| V16.3.3 | 2 | Verify that the application logs the security events that are defined in the documentation and also logs attempts to bypass the security controls, such as input validation, business logic, and anti-automation. | Events in S6 7, including CSRF, rate-limit and validation failures | backend/logging | test `asvs_v16_3_3_*` | Planned |
| V16.3.4 | 2 | Verify that the application logs unexpected errors and security control failures such as backend TLS failures. | Backend TLS and provider failures logged | backend/egress | test `asvs_v16_3_4_*` | Planned |

## V16.4 Log Protection

**Planned control:** Logs in a locked Cloud Logging bucket with 90-day retention; no write access for app identities

**Verification:** Terraform review

| ID | L | Requirement | Control | Location | Verify | Status |
| --- | --- | --- | --- | --- | --- | --- |
| V16.4.1 | 2 | Verify that all logging components appropriately encode data to prevent log injection. | Structured JSON logging; control characters escaped | backend/logging | test `asvs_v16_4_1_*` | Planned |
| V16.4.2 | 2 | Verify that logs are protected from unauthorized access and cannot be modified. | Locked bucket, 90 days, no delete for app identities | Terraform | review | Planned |
| V16.4.3 | 2 | Verify that logs are securely transmitted to a logically separate system for analysis, detection, alerting, and escalation. The aim is to ensure that if the application is breached, the logs are not compromised. | Cloud Logging, separate from the app; alerting in S11 | Terraform | review | Planned |

## V16.5 Error Handling

**Planned control:** Generic user-facing errors; details only in logs

**Verification:** Integration tests

| ID | L | Requirement | Control | Location | Verify | Status |
| --- | --- | --- | --- | --- | --- | --- |
| V16.5.1 | 2 | Verify that a generic message is returned to the consumer when an unexpected or security-sensitive error occurs, ensuring no exposure of sensitive internal system data such as stack traces, queries, secret keys, and tokens. | Generic error bodies; details only in logs | backend/http | test `asvs_v16_5_1_*` | Planned |
| V16.5.2 | 2 | Verify that the application continues to operate securely when external resource access fails, for example, by using patterns such as circuit breakers or graceful degradation. | Timeouts, kill switches, Needs Attention on provider failure | backend/ | test `asvs_v16_5_2_*` | Planned |
| V16.5.3 | 2 | Verify that the application fails gracefully and securely, including when an exception occurs, preventing fail-open conditions such as processing a transaction despite errors resulting from validation logic. | Fail closed: errors stop actions; jobs never marked done on error | backend/ | test `asvs_v16_5_3_*` | Planned |

## V17.1 TURN Server

**Not applicable:** No WebRTC.

| ID | L | Requirement | Control | Location | Verify | Status |
| --- | --- | --- | --- | --- | --- | --- |
| V17.1.1 | 2 | Verify that the Traversal Using Relays around NAT (TURN) service only allows access to IP addresses that are not reserved for special purposes (e.g., internal networks, broadcast, loopback). Note that this applies to both IPv4 and IPv6 addresses. | N/A: see section note | - | - | N/A |

## V17.2 Media

**Not applicable:** No WebRTC.

| ID | L | Requirement | Control | Location | Verify | Status |
| --- | --- | --- | --- | --- | --- | --- |
| V17.2.1 | 2 | Verify that the key for the Datagram Transport Layer Security (DTLS) certificate is managed and protected based on the documented policy for management of cryptographic keys. | N/A: see section note | - | - | N/A |
| V17.2.2 | 2 | Verify that the media server is configured to use and support approved Datagram Transport Layer Security (DTLS) cipher suites and a secure protection profile for the DTLS Extension for establishing keys for the Secure Real-time Transport Protocol (DTLS-SRTP). | N/A: see section note | - | - | N/A |
| V17.2.3 | 2 | Verify that Secure Real-time Transport Protocol (SRTP) authentication is checked at the media server to prevent Real-time Transport Protocol (RTP) injection attacks from leading to either a Denial of Service condition or audio or video media insertion into media streams. | N/A: see section note | - | - | N/A |
| V17.2.4 | 2 | Verify that the media server is able to continue processing incoming media traffic when encountering malformed Secure Real-time Transport Protocol (SRTP) packets. | N/A: see section note | - | - | N/A |

## V17.3 Signaling

**Not applicable:** No WebRTC.

| ID | L | Requirement | Control | Location | Verify | Status |
| --- | --- | --- | --- | --- | --- | --- |
| V17.3.1 | 2 | Verify that the signaling server is able to continue processing legitimate incoming signaling messages during a flood attack. This should be achieved by implementing rate limiting at the signaling level. | N/A: see section note | - | - | N/A |
| V17.3.2 | 2 | Verify that the signaling server is able to continue processing legitimate signaling messages when encountering malformed signaling message that could cause a denial of service condition. This could include implementing input validation, safely handling integer overflows, preventing buffer overflows, and employing other robust error-handling techniques. | N/A: see section note | - | - | N/A |
