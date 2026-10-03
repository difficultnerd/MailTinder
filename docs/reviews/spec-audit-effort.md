# Spec Audit: Effort and Simplifications

3 October 2026. Companion to `spec-audit.md`. Effort is a rough estimate in agent working days: **S** under half a day, **M** 1 to 3 days, **L** 1 to 2 weeks. "Spec" is the edit to the specs; "Build" is the extra code and tests beyond what v1 already needed.

## Totals

| | Spec | Build |
| --- | --- | --- |
| Already removed by Q1 to Q3 (H4 Microsoft half, H6, H7 mostly, H8, M18) | 0 | about 2 weeks saved (page handler service, Microsoft adapter and OneDrive) |
| Remaining highs | about 2 days, mostly done | about 2 to 3 weeks, nearly all of it passkey and key handling (H1 to H3) |
| Remaining mediums | about 2 days | about 1 to 1.5 weeks |
| With the simplifications below | about 3 days total | about 2 to 2.5 weeks total |

Most highs cost little to build. The real cost is the passkey lock, which you chose deliberately; the audit only made it complete. S7 has applied its share (passkey endpoints, `locked` state, two-key design, `API-` IDs), S10 has applied its own, and the product plan thread is part-way through S2 to S6 and S9.

## Simplifications for your yes or no

| # | Choice | Removes or shrinks | Cost of saying yes |
| --- | --- | --- | --- |
| A | **Require PRF; drop the server-held-key fallback.** A passkey without PRF cannot be registered; the app tells the user to use Google Password Manager, iCloud Keychain or Windows Hello | Half of H3 (no account can have mailbox tokens that one Google sign-in unlocks); one code path in H1; the Settings disclosure (AU-03 AC4b) | A friend on Bitwarden for macOS, or another password manager without PRF, cannot join until they switch |
| B | **Only `api`, in session, writes the app folder.** `unsub` stores each job's outcome on the job record; the next Feed load appends it to History | H7's concurrent writers; `unsub` no longer needs Drive access or a second token | History shows an unsubscribe outcome at your next visit, not the moment it runs |
| C | **One session per user.** A new sign-in ends the old one; Settings has "Sign out" only. Replaces the plan thread's current default of up to 10 sessions per user, oldest ended first, with a step-up to end another session | Most of M2 (nothing to list; V7.4.3 and V7.5.2 are met trivially); admin "end this user's sessions" becomes deleting one record | Using the app on phone and laptop at the same time signs the other out |
| D | **Reject rules act only on bulk mail.** A rule keyed on the sender alone matches only messages that carry `List-Unsubscribe` | M5 with no new classifier work | Spammy mail from that sender without the header still reaches the Feed |
| E | **One step-up rule.** A passkey tap within 5 minutes before any account, mailbox, passkey or admin change | M1 becomes one middleware check | One extra tap on those rare actions |
| F | **Mark the register's inapplicable rows N/A now, and add per-row columns only as each feature is built** | Most of M11's spec effort moves into the build, where each task fills its own rows | The register is not fully per-row until the build ends |

My recommendation is yes to all six. Choice A is the only one with a user-visible downside worth weighing.

## Per finding

| ID | Spec | Build | Simpler choice | After choices |
| --- | --- | --- | --- | --- |
| H1 key hierarchy | M (applied in S7, in progress S6) | M | A drops one path | M |
| H2 passkey in S7, S9, S10 | M (applied in S7, S10) | L: WebAuthn plus PRF needs JavaScript interop from Flutter web, as no Flutter package supports PRF | None; this is the passkey lock | L |
| H3 OAuth as a second way in | S | S | A removes the token amplification | S |
| H4 invite token | S | S | Microsoft half gone (Q2) | S |
| H5 DKIM rule | S | S (the DKIM check was needed anyway) | Q3 leaves two methods | S |
| H6 D9, D10 | Done (Q1) | None | | None |
| H7 app folder | S | M | Q2 gives one Drive file; B removes concurrency | S |
| H8 Microsoft scope | Done (Q2) | None | | None |
| M1 step-up | S | S | E | S |
| M2 sessions | S | M | C | S |
| M3 sealed tokens | S (applied in S7) | S | | S |
| M4 body links | S (applied) | None | | None |
| M5 rule over-match | S | S | D | S |
| M6 spoofed block | S | S | | S |
| M7 deletion order | S (applied in S7) | None | | None |
| M8 egress allowlist | S | S | | S |
| M9 S5 schema | S | None | | None |
| M10 gamification API | S (applied in S7) | M (the features you chose) | | M |
| M11 register | M | Spread across tasks | F | S |
| M12 ID clashes | Done (S7) | None | | None |
| M13 CSRF wording | S | None | | None |
| M14 IAM table | S | S (Terraform) | | S |
| M15 privacy notice | M (writing, not code) | None | | M |
| M16 S8 and S11 | M each | None (planned anyway) | Q2 halves S8 | M |
| M17 Jev consent | Done (Q4) | None | | None |
| M18 page handler | Done (Q3) | Saves L | | None |
| M19 job TTL and retries | S | None | | None |
| M20 `eval_id` | S | None | | None |
