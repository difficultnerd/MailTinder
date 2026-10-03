# Gamifying inbox clean-up

Status: DECIDED by James, 3 October 2026.
Replaces: the v1 daily digest as the reason people come back. No push notifications in v1, so every idea here works only when the user opens the app by choice.
Reads with: `docs/CONTEXT.md`, `docs/specs/S2-v1-acceptance-criteria.md` (ST-02), `docs/specs/S5-data-inventory.md`, `docs/specs/S9-functional-screens.md` (7.4 Stats).
Edits to CONTEXT.md, S2, S5 and S9 go through the product plan thread.

## Decision (James, 3 October 2026)

v1 includes ideas 1 to 8: inbox meter, juicy swipe feedback, end-of-round card, backlog years as levels, mail stopped counter, achievements, blitz mode and boss senders. The design goal is that the user feels they are making progress. Out of v1: the weekly streak and the brag card (revisit later), leaderboards and carbon or time-saved figures (rejected).

Boss senders in v1 use the sender stats the app already builds while swiping, so a boss appears once the app has seen enough of that sender's mail. No metadata scan. The health bar counts that sender's inbox mail via one provider query when the boss is shown.

## Original recommendation

Build six things for v1: the inbox meter, juicy swipe feedback, the end-of-round card, backlog years as levels, a "mail stopped" counter and a short list of achievements. None adds a Firestore collection or a log field. Two add a few small values to the encrypted Drive app folder file that S5 already defines. Keep streaks out of v1, or use a forgiving weekly streak if James wants one. Leave leaderboards out entirely.

## What the evidence says

- **Progress you can see beats points.** The best email-specific motivator is the inbox count going down. Gmail returns `messagesTotal` for the INBOX label and Microsoft Graph returns `totalItemCount` per folder, so the app can show it live with no stored state.
- **Streaks work, but mainly for daily habits.** Duolingo ran over 600 streak experiments; retention jumps sharply until day 7 then flattens, and two streak freezes beat one while three were no better than two ([Lenny's Podcast summary](https://www.getrecall.ai/summary/lennys-podcast/behind-the-product-duolingo-streaks-or-jackson-shuttleworth-group-pm-retention-team)). Duolingo drives those streaks with reminders. Mail Tinder has no push and no digest, so a daily streak would break silently and then punish the user on their next visit. Inbox clean-up is also a chore people do in bursts, not a daily practice.
- **Loss aversion cuts both ways.** Streak anxiety and "the streak becomes the point" are well-documented failure modes ([NerdSip](https://nerdsip.com/blog/gamification-gone-wrong-when-streaks-become-the-point), [Routinery](https://www.routinery.app/blog/micro-rewards-vs-streaks-adherence-science)). For a tool whose job is reducing stress, guilt mechanics are a poor fit.
- **Do not use carbon savings.** Cleanfox frames deletion as CO2 saved. Deleting 1,000 stored emails saves about 5 g CO2e, while 30 minutes of laptop use to do it costs 5 to 28 g depending on the grid ([The Conversation](https://theconversation.com/can-sending-fewer-emails-or-emptying-your-inbox-really-help-fight-climate-change-193822), [bertptrs](https://bertptrs.nl/2024/08/24/deleting-emails-will-not-save-the-planet.html)). A claim like that would undermine a product built to show clients it is trustworthy.
- **Swipe apps already hold most of the fun.** Tinder's appeal is fast binary choice with instant feedback. Mail Tinder gets that for free; the job is to make each swipe feel good and to show the user what their swipes added up to.

## Ranked ideas

Fun and cost are relative scores out of 5. "Data" says what the idea stores and where.

| Rank | Idea | Fun | Cost | Data |
| --- | --- | --- | --- | --- |
| 1 | Inbox meter | 4 | 1 | None. Live provider counts |
| 2 | Juicy swipe feedback | 4 | 2 | None. Front end only |
| 3 | End-of-round card | 4 | 1 | None. Browser memory only |
| 4 | Backlog years as levels | 5 | 2 | None new. Uses the Feed cursor S5 already stores |
| 5 | Mail stopped counter | 5 | 2 | One integer per reject rule, Drive file |
| 6 | Achievements | 3 | 2 | About 20 unlock flags, Drive file |
| 7 | Blitz mode (60-second round) | 4 | 2 | None |
| 8 | Boss senders | 5 | 3 | None new. Uses sender stats S5 already stores |
| 9 | Weekly streak | 2 | 2 | Week of last clean plus a count, Drive file |
| 10 | Brag card | 3 | 2 | None stored. Image made in the browser |
| No | Leaderboards or friend comparison | 3 | 4 | New Firestore collection, cross-user data |
| No | Carbon or "time saved" figures | 1 | 1 | None, but the claims are weak |

### 1. Inbox meter

A bar or dial on the Feed showing the inbox count across all mailboxes, with the change since the start of this session ("12,431, down 214 today"). Gmail and Graph both return folder totals in one call, so the app fetches it on open and after each batch of swipes. The meter is the closest thing to Inbox Zero the app can promise, and it shows real progress on a 20-year backlog.

### 2. Juicy swipe feedback

Card physics, a distinct sound and haptic per direction, a "flush" for a three-poop reject, a soft chime for a keep, a short combo counter for rapid swipes and confetti at milestones (every 100 cleared). Nothing is stored. Cost sits in front-end polish, which CONTEXT.md already names as the near-term priority. Sounds default off on web; haptics need the Vibration API, which iOS Safari does not support, so treat haptics as a bonus on Android.

### 3. End-of-round card

When the user leaves the Feed or hits a natural pause (every 50 swipes, or the "up to date" divider), show a card: cleared, kept, filed, senders unsubscribed, senders blocked. Held in browser memory for the session, which S5 already allows for the undo stack. This replaces the digest's job of telling the user what they achieved, at the moment they achieved it.

### 4. Backlog years as levels

The Feed already works backwards through older mail. Turn each calendar year into a level: "Level 2023: 1,840 left". Clearing a year shows a level-complete screen and the next year unlocks. The year count comes from a provider query (`before:` and `after:` in Gmail, a date filter in Graph); the position comes from the Feed cursor already in the Drive file. Fits James's own goal of pacing through 20 years and turns an endless Feed into chapters with an end.

### 5. Mail stopped counter

When a reject creates an unsubscribe or a reject-list rule, the backend asks the provider how many messages matched that sender and List-Id in the past 90 days (Gmail `resultSizeEstimate`, Graph `$count`) and stores the yearly rate as one integer on the rule. The Stats screen then shows "About 3,200 emails a year stopped", and History can show it per sender. This is the most honest, meaningful number the app can show, because unsubscribing is the product's core promise.

- Data: one integer per rule in the encrypted Drive file. It reveals volume from a sender the rule already names, so it adds no new class of personal data (C2, same as the rule). No Firestore change.
- The provider query runs in the request that handles the reject, so no new scope or background job.

### 6. Achievements

A small, fixed list unlocked locally: first unsubscribe, 100 senders silenced, a year cleared, 1,000 cleared, first filing category, first blocked person, ten unsubscribes in a round. Shown in Stats. Store only the achievement ID and unlock date in the Drive file. Keep the list short; a long badge grid becomes clutter.

### 7. Blitz mode

A 60-second timed round with a score. Fast and fun, but speed raises the error rate on rejects. Safeguards: the existing unsubscribe delay and undo still apply; cards with a personal badge are excluded from blitz rounds; the block prompt is held until the round ends. No stored data.

### 8. Boss senders

The senders with the most mail in the inbox become "bosses" with a health bar ("Temu: 1,243 left"). Rejecting one "defeats" it, and with the v2 retroactive clean-up the health bar drains as historic mail goes. Fun is high, but finding top senders needs either the sender stats the app builds while swiping (available, but only for mail already seen) or a costly scan of message metadata. Best as v1.1, or once v2 bulk reclassification exists.

### 9. Weekly streak (if wanted)

If James wants a streak, count weeks with at least one session, not days, and include one free miss per month. This matches burst behaviour and survives the lack of reminders. Data: the ISO week of the last session and the streak length in the Drive file. These two values reveal usage pattern (C2); acceptable inside the user's own encrypted file, but they add to what a user's Drive holds. Recommend leaving this out of v1 and seeing whether the meter and levels are enough.

### 10. Brag card

A share button on the end-of-round card or a level-complete screen that draws an image in the browser with numbers only ("Cleared 2019. 4,102 emails, 87 senders gone"). The user saves or shares it themselves; nothing goes to the server. Sender names stay off the image by default, since a shared screenshot of mail senders is a privacy leak the user may not notice.

## Rejected

- **Leaderboards and friend comparison.** Tempting with 20 trial friends, but it needs a new server-side collection holding per-user activity, cross-user reads, access-control tests and anti-cheat (scores from the Drive file are user-controlled, so they cannot be trusted for ranking). S5's schema test would rightly fail the build. The brag card gets most of the social fun with none of that.
- **Carbon and time-saved figures.** Carbon is covered above. "Time saved" needs an assumed minutes-per-email figure with no good source; an invented number undermines the stats around it.
- **Daily streaks.** Covered above: no reminders means silent breaks and guilt on return.

## Data and ASVS L2 impact

| Idea | Firestore | Drive app folder file | Logs | Browser |
| --- | --- | --- | --- | --- |
| Inbox meter, feedback, end-of-round, blitz | None | None | None | Memory only |
| Levels | None | Uses existing Feed cursor | None | Memory only |
| Mail stopped | None | Adds one integer per rule (C2) | None | Memory only |
| Achievements | None | Adds ID and date per unlock (C2) | None | Memory only |
| Weekly streak | None | Adds week and count (C2) | None | Memory only |
| Leaderboards (rejected) | New collection (C2) | None | New events | Memory only |

- Nothing recommended adds a Firestore collection, a log field or browser storage, so S5's schema test, the log-scanning test (LOG-1) and the ASVS V14.3 browser-storage rule stand unchanged.
- New Drive file fields need rows in S5's app folder table and are covered by the existing encryption and deletion path (AU-06).
- A user can tamper with their own Drive file, but the only effect is on their own scores. No other user or server decision depends on these values, so there is no integrity requirement beyond what the file already has.
- No new provider scopes. The meter and level counts use read calls the app already makes.

## Spec changes for the decided set

For the product plan thread to make:

- **CONTEXT.md:** replace the open gamification item with the chosen list.
- **S2:** extend ST-02 with acceptance criteria for the meter, end-of-round card, levels, mail stopped and achievements; add criteria for blitz mode (60-second round, personal-badge cards excluded, block prompt held until the round ends, unsubscribe delay and undo unchanged) and boss senders (drawn from existing sender stats, health bar from one provider count query).
- **S5:** add the mail-stopped integer per rule and the achievements list to the app folder table.
- **S9:** add the meter, level banner and boss banner to the Feed, a blitz mode entry point and round screen, the end-of-round and level-complete overlays, and the achievements list to 7.4 Stats; replace "current streak or similar fun figure".
