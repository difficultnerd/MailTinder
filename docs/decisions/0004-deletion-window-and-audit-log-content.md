# ADR 0004: Account deletion is immediate crypto-shredding with a known one-hour read window; audit logs may hold operator identities

Status: accepted (owner decisions, 10 October 2026, recorded in the factory's decision memos; source: the owner's replies "D1 immediate" and "D2 accept" to the Terraform decision memo, in this repository's PR #20 review discussion)
Date: 10 October 2026

## Context

The Terraform foundation (T-1102a, T-1102b) configures Firestore without point-in-time recovery or backups (the 4 October trial rule) and routes audit entries for KMS and Secret Manager to a locked 90-day log bucket. Review of PR #20 found that (1) Firestore still allows reads of historical document versions for about one hour even with point-in-time recovery off, so "no older copy survives deletion" was stronger than the platform provides, and (2) audit entries carry the calling principal (an operator's email) and caller IP, which sits uneasily with the "no addresses in logs" rule.

## Decision

1. **Deletion.** Account deletion destroys the user's wrapped `data_key` at once (crypto-shredding). The trial keeps no backups, no point-in-time recovery and no scheduled exports, and there is no recovery window. Residual window: for about one hour after a change, historical versions of the document can be read by an identity that has Firestore read access; using such a copy also needs KMS decrypt on the shared key encryption key, which only the `api`, `unsub` and `worker` service accounts hold (and which would let them read the live key anyway). The window is accepted. Revisit before general release (backups or recovery would be a new decision).
2. **Audit logs.** Operator identities (email) and caller IP in the KMS and Secret Manager audit copy in the locked 90-day bucket are accepted. This covers operators (the owner and service accounts), never end users. The sink copies all `cloudaudit` entries of those two services (admin activity, data read, data write), not only decrypts and secret reads.

## Consequences

- S5 (DEL-2, audit-log paragraph), S6 5 (Deletion), S11 and the backlog task files that restated the absolute claim cite this ADR.
- DEL-2 tests must either wait out the window or assert only what the platform guarantees (key destroyed; ciphertext undecryptable after the window); T-803 says which.
- The one-hour figure is Google platform behaviour (Firestore document version retention); it is not controlled by this repository, so it is re-checked when the foundation is first applied.
