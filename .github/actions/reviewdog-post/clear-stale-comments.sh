#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 2 ]]; then
  echo "usage: clear-stale-comments.sh <pr_number> <tool_name>" >&2
  exit 2
fi

pr_number="$1"
tool_name="$2"
repo="${GITHUB_REPOSITORY:?}"
script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

if ! command -v gh >/dev/null; then
  echo "::error::gh CLI is required to clear stale reviewdog comments"
  exit 1
fi

export GH_TOKEN="${REVIEWDOG_GITHUB_API_TOKEN:-${GITHUB_TOKEN:-}}"

gh api --paginate "repos/${repo}/pulls/${pr_number}/comments" |
  python3 "${script_dir}/clear-stale-comments.py" "$tool_name" "$repo"
