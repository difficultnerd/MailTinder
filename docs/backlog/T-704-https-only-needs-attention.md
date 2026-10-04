# T-704: Https-only links to Needs Attention

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M7 | sonnet | about 200 lines of code plus tests | T-605, T-701 |

**Read only these spec sections:** S2 UN-04 AC6, UN-05 AC1, SW-03 AC2 and AC5, UN-02 AC2 (`docs/specs/S2-v1-acceptance-criteria.md`), S3 "Message classes" table row `list` (`docs/specs/S3-domain-model.md`), S6 6 bullets "One DKIM rule" and "Https-only" (`docs/specs/S6-security.md`), S7 5.5 `outcome` row `trashed_unsubscribe_manual` and S7 5.8 bullets on `reason_code` and `link` (`docs/specs/S7-api-contract.md`), S10 5 corpus rows "https without one-click" and "`http://` (plain) unsubscribe link" (`docs/specs/S10-test-strategy.md`), ASVS register row V1.2.2 (`docs/security/asvs-l2-register.md`). Nothing else is needed.

## Goal

When a user rejects a list message whose DKIM-covered `List-Unsubscribe` offers only an https link (no one-click, no mailto), no job is created; the message is trashed, a reject rule is created and a Needs Attention item `https_only_unsubscribe` is raised with an "Open unsubscribe page" link taken only from that header. This task adds the shared link sanitiser `safe_link` and wires the item into the reject path, using T-102's `HeaderRules::unsubscribe_route` to pick the method.

## Files

| Action | Path | What |
| --- | --- | --- |
| Create | `backend/crates/svc-common/src/links.rs` | `safe_link` |
| Change | `backend/crates/svc-common/src/needs_attention.rs` | `raise_item` calls `safe_link` on the link |
| Change | `backend/crates/api/src/services/reject.rs` (T-605's reject service; use its real path) | On `UnsubscribeRoute::ManualLink` call `raise_item` |
| Create | `backend/crates/api/tests/reject_https_only.rs` | Service integration tests over the corpus |

## Types and signatures

```rust
// svc-common/src/links.rs
pub const MAX_LINK_CHARS: usize = 2048;   // [DEFAULT] longer links are almost always tracking payloads
/// Returns Some only for: scheme https, a host, no username or password, at most MAX_LINK_CHARS,
/// no ASCII control characters or whitespace anywhere in the input.
pub fn safe_link(raw: &str) -> Option<Url>;
```

`HeaderRules::unsubscribe_route(facts) -> UnsubscribeRoute` (`OneClick(Url)`, `Mailto(MailtoTarget)`, `ManualLink(Option<Url>)`, `None`) comes from T-102. It already applies the precedence one-click, then mailto, then the manual https link, and gives `ManualLink(None)` for a plain `http` link. Do not write a second planner.

## Algorithm

Reject path (in T-605's reject service, after trash and rule creation, for class `list` only):

1. `match HeaderRules::unsubscribe_route(&meta.facts)` with every arm listed:
   - `OneClick` or `Mailto`: unchanged T-605 behaviour (queue the job; outcome `trashed_unsubscribe_queued`).
   - `ManualLink(link)`: create no job; call `raise_item(NewItem { user, mailbox, sender_display: &meta.from_display, link: link.as_ref(), reason: HttpsOnlyUnsubscribe })`; outcome `trashed_unsubscribe_manual`.
   - `None`: create no job and no item; outcome `trashed_list_no_unsubscribe`.
2. Never read a link from the message body or preview. The only input is `meta.facts`.
3. If `raise_item` fails (store error), the reject still succeeds (trash and rule are done), the error is logged without values, and the response outcome stays `trashed_unsubscribe_manual` `[DEFAULT]`; XC-04 is met because the next reject or the History entry shows the action. Do not retry inside the request.

`raise_item` change: before encrypting, pass the link through `safe_link(link.as_str())`; a link that fails becomes `None`. The app hides "Open unsubscribe page" when `link` is null (T-1005).

## Acceptance criteria

| ID | Behaviour (one line) |
| --- | --- |
| UN-04 AC6 | Https-only DKIM-covered header: no job, trash, reject rule, item with "Open unsubscribe page" from the header only |
| UN-05 AC1 | The item carries the sender, the link and the reason |
| SW-03 AC2 | An https link without one-click raises a Needs Attention item instead of a job |
| V1.2.2 | Only `https` links are stored on items; `javascript:`, `data:` and `http:` are dropped |

## Tests that must pass

- `un_04_ac6_https_only_creates_no_job` (service integration: corpus case "https without one-click, DKIM covers")
- `un_04_ac6_item_raised_with_open_unsubscribe_page` (service integration: reason `https_only_unsubscribe`, link equals the header URL)
- `un_04_ac6_trashed_and_reject_rule_created` (service integration)
- `un_04_ac6_link_only_from_dkim_header` (service integration: the message body holds a different https link; the item's link is the header's)
- `un_04_ac6_no_dkim_cover_no_item` (service integration: header present but not covered; zero items, zero jobs)
- `un_04_ac6_plain_http_link_never_fetched_item_has_no_link` (service integration: egress fake records zero requests; item link is null)
- `un_05_ac1_https_only_item_has_sender_link_reason` (service integration: decrypt the stored item and check all three)
- `sw_03_ac2_https_only_raises_item_not_job` (service integration)
- `asvs_v1_2_2_only_https_links_stored` (unit on `safe_link`: `https://example.com/u` kept; `http://example.com/u`, `javascript:alert(1)`, `data:text/html,x`, `https://user:pw@example.com/`, a 2,049-character URL, a URL with a space or newline: all `None`)

## Edge cases and traps

- Do not parse the body for links, ever (S2 SW-03 AC5, S6 6). The body is not an input to this code.
- The item's `sender_display` comes from the From display name (plain text, already cleaned by `plain_text` in T-602c), never the address.
- Do not log the link or the sender display.
- A plain `http:` link must never be fetched, not even a HEAD request.
- `UnsubscribeRoute` has no `_` arm anywhere it is matched.
- If T-605 already raises this item with its own code, replace that code with the call above so there is one path, and keep T-605's tests green.

## Out of scope

- Opening the link (the user does that in the app, T-1005); the v2 page handler.
- The Needs Attention list and actions: T-705.
- DKIM coverage itself: T-406. Trash and rule creation: T-605.

## Done when

- The tests above pass and every required check is green (S10 10.1).
- Definition of done in S10 10.4.
