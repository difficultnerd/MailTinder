#!/usr/bin/env bash
# Apply branch protection to main using the GitHub CLI (free). Run once per repo created
# from this template; template repos do not copy branch protection settings.
#
# Usage: tools/apply_branch_protection.sh [owner/repo] [branch]
#
# Requires: gh authenticated with admin rights on the repo.
# Note: on GitHub Free, branch protection works on public repos only. Private repos
# need GitHub Pro/Team, so make the repo public or accept the gap.
set -euo pipefail

repo="${1:-$(gh repo view --json nameWithOwner --jq .nameWithOwner)}"
branch="${2:-main}"

# Names must match the job ids in .github/workflows/{ci,security,privacy}.yml.
contexts='["rust","dart","language-policy","gitleaks","semgrep","cargo-audit","cargo-deny","dart-licenses","privacy-checks"]'

gh api --method PUT "repos/${repo}/branches/${branch}/protection" --input - <<JSON
{
  "required_status_checks": { "strict": true, "contexts": ${contexts} },
  "enforce_admins": true,
  "required_pull_request_reviews": null,
  "restrictions": null,
  "allow_force_pushes": false,
  "allow_deletions": false,
  "required_linear_history": false,
  "required_conversation_resolution": true
}
JSON

echo "Branch protection applied to ${repo}@${branch}."
