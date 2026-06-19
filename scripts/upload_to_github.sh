#!/usr/bin/env bash
set -euo pipefail

REPO_URL="${REPO_URL:-https://github.com/lkr999/stock_2026.git}"
BRANCH="${BRANCH:-main}"
COMMIT_MESSAGE="${1:-Initial upload}"

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

if ! git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  git init -b "$BRANCH"
fi

current_branch="$(git branch --show-current || true)"
if [[ -z "$current_branch" ]]; then
  git checkout -b "$BRANCH"
elif [[ "$current_branch" != "$BRANCH" ]]; then
  git branch -M "$BRANCH"
fi

for secret_file in .env backend/.env frontend/.env; do
  if git ls-files --error-unmatch "$secret_file" >/dev/null 2>&1; then
    echo "Refusing to continue: $secret_file is already tracked by git."
    echo "Remove it from the index first, for example: git rm --cached $secret_file"
    exit 1
  fi
done

if git remote get-url origin >/dev/null 2>&1; then
  git remote set-url origin "$REPO_URL"
else
  git remote add origin "$REPO_URL"
fi

git add .gitignore README.md CANDLESTICK_TRADING_SYSTEM.md CHART_AND_SIGNAL_ANALYSIS.md backend files frontend run_dev.sh scripts/upload_to_github.sh

if git diff --cached --quiet; then
  echo "Nothing to commit."
else
  git commit -m "$COMMIT_MESSAGE"
fi

git push -u origin "$BRANCH"
