#!/usr/bin/env bash
# Turn on "require signed commits" for main. Usage: tools/require_signed_commits.sh [owner/repo] [branch]
set -euo pipefail
repo="${1:-$(gh repo view --json nameWithOwner --jq .nameWithOwner)}"
branch="${2:-main}"
gh api --method POST "repos/${repo}/branches/${branch}/protection/required_signatures" \
  -H "Accept: application/vnd.github+json" >/dev/null
echo "Signed commits required on ${repo}@${branch}."
