#!/usr/bin/env bash
# merge-build-to-main.sh — fast-forward agent/overnight-build into main after a build batch.
# Called by the build driver cron at the end of each run. Safe to run repeatedly:
# if there's nothing new, it's a no-op.
#
# Rules:
#  - Only fast-forward merges (main must be an ancestor of the build branch).
#    If main has diverged, do NOT force — report and exit non-zero so the driver
#    knows to stop and ask the user.
#  - Never touches the working tree of the build branch (it stays checked out).
set -euo pipefail

REPO="${1:-/home/ubuntu/MailTinder}"
BUILD_BRANCH="agent/overnight-build"
MAIN_BRANCH="main"

cd "$REPO"

# Make sure we're looking at the latest remote state.
git fetch origin "$BUILD_BRANCH" "$MAIN_BRANCH" >/dev/null 2>&1 || true

# Nothing to do if the build branch hasn't advanced past main.
if git merge-base --is-ancestor "$MAIN_BRANCH" "$BUILD_BRANCH"; then
  if [ "$(git rev-parse "$MAIN_BRANCH")" = "$(git rev-parse "$BUILD_BRANCH")" ]; then
    echo "merge-build-to-main: main already up to date with $BUILD_BRANCH — nothing to do."
    exit 0
  fi
else
  echo "merge-build-to-main: main has DIVERGED from $BUILD_BRANCH — refusing to force. Needs human review." >&2
  exit 1
fi

# Fast-forward main to the build branch head, then push.
git push origin "$BUILD_BRANCH:$MAIN_BRANCH"
echo "merge-build-to-main: fast-forwarded $MAIN_BRANCH to $(git rev-parse --short "$BUILD_BRANCH") ($BUILD_BRANCH)."
