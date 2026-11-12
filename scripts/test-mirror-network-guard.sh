#!/usr/bin/env bash
# Pure policy tests: no Docker, network requests, or real package/service commands.
set -euo pipefail
root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
# shellcheck source=scripts/lib/mirror-network-guard.sh
source "$root/scripts/lib/mirror-network-guard.sh"

check() {
  local expected=$1 permission=$2 actions=$3 event=$4 ref=$5 status=0
  LKIT_ALLOW_MIRROR_NETWORK=$permission GITHUB_ACTIONS=$actions \
    GITHUB_EVENT_NAME=$event GITHUB_REF=$ref \
    lkit_require_mirror_network 2>/dev/null || status=$?
  [[ $status == "$expected" ]] || {
    echo "Unexpected policy result: $permission/$actions/$event/$ref: $status" >&2
    exit 1
  }
}

check 0 1 true push refs/heads/ci-mirror
check 2 0 true push refs/heads/ci-mirror
check 2 '' true push refs/heads/ci-mirror
check 2 true true push refs/heads/ci-mirror
check 2 1 false push refs/heads/ci-mirror
check 2 1 true push refs/heads/main
check 2 1 true push refs/heads/dev
check 2 1 true push refs/heads/ci-mirror-other
check 2 1 true push refs/tags/ci-mirror
check 2 1 true pull_request refs/pull/1/merge
check 2 1 true pull_request refs/heads/ci-mirror
check 2 1 true workflow_dispatch refs/heads/ci-mirror
check 2 1 true schedule refs/heads/ci-mirror
check 2 '' '' '' ''
echo 'Mirror network guard: 14 policy cases passed'
