# Optional layer: data handling and privacy by design

Use for repos that make a no-data-retention commitment.

Install: `optional/install.sh privacy` from the repo root, then:

1. The installer adds `backend/clippy.toml`; merge it by hand if you already have one (existing files are never overwritten).
2. Edit `PRIVACY.md` to state your real commitment.
3. Add `privacy-checks` to the required status checks in `tools/apply_branch_protection.sh`.
