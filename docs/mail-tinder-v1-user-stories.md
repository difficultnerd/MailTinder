# Mail Tinder - Version 1 User Stories

Oct 3, 2026

Superseded by docs/specs/S2 and CONTEXT decisions.

## Overview and scope

Mail Tinder is a Tinder-style swipe interface for triaging email across providers. Core premise: swipe right to leave an email where it is, swipe left to reject and trigger unsubscribe, swipe up to actively file it (super-like), swipe down to skip with no action. Built provider-agnostic from the start (adapter pattern), launching with Gmail and Outlook. Architected to scale to public use eventually, but the near-term goal is a polished proof of concept: James first, then up to about 20 friends briefly, then client demos.

Version 1 scope is the core swipe loop, live (non-retroactive) unsubscribe, and the filing/learning behaviour. The retroactive historical classifier that bulk-cleans a sender's past mail is explicitly deferred to version 2 (see Deferred section) to avoid shipping a risky destructive feature before the core loop is proven.

## Core swipe mechanics

1. As a user, I want to see one email at a time as a card on the Feed screen, showing sender, subject line, a body preview, and a spam confidence score, so I can decide quickly what to do with it.
2. As a user, I want to swipe right (or tap an equivalent button) to leave an email exactly where it is in my mailbox, so low-effort triage doesn't require a decision.
3. As a user, I want to swipe left to reject an email, which triggers an unsubscribe attempt, so unwanted senders stop reaching me.
4. As a user, I want to swipe up to actively file an email (super-like), so I can pull out things I want to keep and organise.
5. As a user, I want to swipe down to skip an email with no action taken, so I can defer a decision without being forced to choose.
6. As a user, I want a persistent undo button on the Feed screen, so I can instantly reverse my last swipe if I made a mistake.
7. As a user, I want the Feed to surface new incoming mail first in an endless-scroll style, rather than my entire mailbox history at once, so triage never feels overwhelming.
8. As a user, I want to be able to stop swiping at any point and have the Feed resume where I left off next time I open the app, with no daily cap imposed on me.

## Provider connection and authentication

1. As a user, I want to log in using my existing Gmail or Outlook account via OAuth, so I don't need a separate username and password.
2. As a new user, I want my OAuth login to also grant the app the mail access it needs, so account creation and mailbox connection happen in a single step.
3. As a product owner, I want email provider integrations built as separate, swappable back-end modules (adapter pattern), so additional providers can be added later without reworking the core app.
4. As the product owner, I want version 1 to ship with Gmail and Outlook support only, so we can prove the core loop before expanding provider coverage.
5. As a product owner, I want access to the app restricted to invited users only, so it can be trialled with a small group of friends without being publicly discoverable.
6. As a user, once I connect an account, I want the app to start pulling in new mail going forward, not my entire historical inbox at once.
7. As a user, I want a sub-area within Settings for managing connected provider accounts (adding or removing Gmail or Outlook logins), so I can control my connections without leaving the app's main navigation.

## Unsubscribe behaviour

1. As a user, when I swipe left on an email with a clean List-Unsubscribe header, I want the app to action the unsubscribe immediately in the background, with no further input from me.
2. As a user, when I swipe left on an email with no clean unsubscribe mechanism, I want an agent to attempt to work out how to unsubscribe on its own (following links, submitting forms, etc.), so I don't have to do it manually.
3. As a user, when the unsubscribe agent gets stuck (e.g. hits a CAPTCHA or an unworkable flow), I want it to stop and flag the item for my attention rather than fail silently or loop indefinitely.
4. As a product owner, I want the technical feasibility of the autonomous unsubscribe agent researched and validated early, since it is the highest-uncertainty piece of version 1 (see the separate research brief).

## Needs attention and notifications

1. As a user, I want a dedicated Needs Attention screen, accessible from the bottom tab bar, listing every item an agent couldn't resolve on its own (e.g. a stuck unsubscribe), so I can review and resolve them in one place.
2. As a user, I want to be notified (push notification and/or daily digest) when an agent needs my help, so stuck items don't sit unnoticed.
3. As a product owner, I want the daily digest and notifications scoped specifically to agent help requests, not a general activity summary, keeping the feature focused.

## Super-like filing and keep-learning

1. As a user, when I swipe up on an email, I want the app to suggest a filing category based on patterns it's recognised from my past behaviour (e.g. tax invoices filed as financial records), so I don't have to manually categorise everything.
2. As a user, the first time the app sees a new type of content, I want it to ask me what I want to call the category, so my filing system reflects how I actually think about my mail.
3. As a user, I want the app's filing suggestions to get quicker and more confident over time (less friction, a one-tap confirm), but I always want to see its guess, plus alternates if it's wrong, rather than it filing things fully silently.
4. As a user, I want a dedicated screen in the bottom tab bar for the super-like filing area, where I can see and manage what's been filed.
5. As a user, when I swipe right (keep) on emails, I want the app to notice patterns over time (e.g. I always keep Apple invoices) and, once a pattern emerges, suggest filing similar future emails the same way, so low-effort keeps can still get organised without me doing the work up front.
6. As a product owner, I want swipe-right's filing suggestion to stay lower-friction and passive compared to swipe-up, which is the deliberate, immediate filing action, so the two gestures remain meaningfully distinct.

## Email card UI and navigation

1. As a user, I want each email card to show the sender, subject line, a preview of the body, and a spam confidence score, so I can judge it at a glance before swiping.
2. As a user, I want a bottom tab bar with four destinations: Feed, the super-like filing area, Needs Attention, and Settings, so the app's main functions are always one tap away.
3. As a user, I want history and stats available inside Settings rather than as their own tab, keeping the main navigation lean.

## Deferred to version 2

1. As a user, when I reject and unsubscribe from a marketing email, I want the app to retroactively scan historical mail from that sender and, using a content classifier, remove only the marketing-type messages (not receipts, invoices, or substantive content), moving them to trash rather than deleting permanently.
2. As a user, when a keep pattern is confirmed for a sender (e.g. I always keep Apple invoices), I want the option to apply that filing decision retroactively across historical mail from that sender too.
3. As a product owner, this retroactive bulk-action classifier is deliberately held back from version 1 and shipped as a fast-follow, once the core swipe loop and live unsubscribe are proven safe and reliable.
