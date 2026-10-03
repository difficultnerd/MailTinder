# Mail Tinder - Unsubscribe Agent Research Brief

Oct 3, 2026

Superseded by docs/specs/S2 and CONTEXT decisions.

## Purpose

Mail Tinder's reject gesture needs to unsubscribe the user from a sender. Where a clean List-Unsubscribe header exists, that's a solved problem. This brief scopes the harder case: when no clean header exists and an autonomous agent must work out how to unsubscribe itself (following links, navigating pages, submitting forms). This is the single highest-uncertainty piece of the version 1 build, and this document exists to get a feasibility assessment and a recommended approach before it's committed to as a confident v1 feature rather than an open experiment.

## Scope of the fallback agent

When a reject swipe has no List-Unsubscribe header to action directly, the agent must, on its own:

- Identify the unsubscribe path from the email itself (a link, a button, instructions in the body).
- Navigate to and interact with external web pages, which may include forms, confirmation steps, or login walls.
- Determine when it has successfully completed the unsubscribe versus when it has hit a dead end.
- Stop and clearly flag the item for human help when it cannot complete the task (e.g. CAPTCHA, broken flow, ambiguous page), rather than looping or failing silently.

Agents are expected to fail sometimes. Failure plus a clear request for human help is an acceptable outcome; silent failure is not.

## Known constraints already decided

- Clean List-Unsubscribe header cases are out of scope here; they're actioned directly, no agent needed.
- The agent result needs to feed a Needs Attention queue in-app when it gets stuck, with push notification and daily digest alerting.
- Provider architecture is adapter-based (Gmail and Outlook first), so the agent's integration point should assume it can be triggered per-provider, not tied to one mailbox implementation.
- Version 1 timeline is near-term (friends trial coming soon), so the research should flag anything that would be a hard blocker to shipping this specific feature in v1, versus something viable as a v2 fast-follow.

## Questions for the research agent

1. What browser automation or agentic web-navigation approaches are viable today for an unsubscribe task (e.g. computer-use style agents, headless browser plus LLM-directed navigation, existing open-source tooling)?
2. What is the realistic success rate for autonomous unsubscribe across common real-world patterns (simple confirmation pages, preference-centre pages, login-gated unsubscribe, forms requiring an email address to be typed in)?
3. What are the most common failure modes (CAPTCHAs, bot detection, broken or infinite flows, ambiguous success states) and how reliably can the agent detect that it has failed versus succeeded?
4. What are the cost and latency implications of running this kind of agent per-unsubscribe, at the scale of a single user's daily rejects?
5. Are there legal, safety, or email-provider policy considerations (e.g. automated interactions with third-party sites, rate limiting, ToS issues) worth flagging?
6. Based on the above, is an autonomous fallback agent realistic to ship in version 1, or does it warrant staging further (e.g. a simpler v1 that flags all non-header cases for manual action, with full autonomy as a v2 upgrade)?
