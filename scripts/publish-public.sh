#!/usr/bin/env bash
# Publish the public GitHub mirror of this repository.
#
# The full repository keeps the offline research corpus (il/, analysis/,
# reference/ upstream RadioDJ binaries and decompiled output). The public
# mirror is a separate clone whose entire history has those paths stripped
# and whose commit authors are mapped via scripts/publish-mailmap.
#
# The full repo is never rewritten. Re-run this script after new commits
# to republish: it re-fetches master from the full repo, re-strips, and
# force-pushes. Re-running git filter-repo with --force is cumulative, so
# the rewrite stays stable.
#
#   PUBLIC_DIR=/other/dir GITHUB_REPO=user/name scripts/publish-public.sh
set -euo pipefail

FULL_REPO="$(cd "$(dirname "$0")/.." && pwd)"
PUBLIC_DIR="${PUBLIC_DIR:-$HOME/Dev/radiocraft-public}"
GITHUB_REPO="${GITHUB_REPO:-zoltarffai/radiocraft}"

if [ ! -d "$PUBLIC_DIR/.git" ]; then
  git clone "$FULL_REPO" "$PUBLIC_DIR"
fi

cd "$PUBLIC_DIR"
git fetch "$FULL_REPO" master:refs/heads/upstream-master --force
git update-ref refs/heads/master refs/heads/upstream-master
git update-ref -d refs/heads/upstream-master

git filter-repo --force --invert-paths \
  --path il --path analysis --path reference \
  --mailmap "$FULL_REPO/scripts/publish-mailmap"

# filter-repo drops the origin remote on purpose; wire it back each run.
git remote remove origin 2>/dev/null || true
git remote add origin "https://github.com/$GITHUB_REPO.git"

gh repo view "$GITHUB_REPO" >/dev/null 2>&1 || \
  gh repo create "$GITHUB_REPO" --public

git push --force origin master
git push --tags --force origin 2>/dev/null || true

echo "published $(git rev-parse --short master) -> github.com/$GITHUB_REPO (master)"
