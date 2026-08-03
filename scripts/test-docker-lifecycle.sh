#!/usr/bin/env bash
set -euo pipefail

case $(uname -s):$(uname -m) in
  Linux:x86_64) ;;
  Linux:aarch64|Linux:arm64)
    if [[ ${LKIT_E2E_ALLOW_ARM:-} != 1 ]]; then
      echo "Docker functional E2E is supported locally only on Linux x86_64; use CI for aarch64" >&2
      exit 2
    fi
    ;;
  *)
    echo "Docker functional E2E requires Linux x86_64 or the CI ARM runner" >&2
    exit 2
    ;;
esac

if ! docker info >/dev/null 2>&1; then
  echo "Docker is required for the lifecycle E2E" >&2
  exit 1
fi
if ! docker compose version >/dev/null 2>&1; then
  echo "Docker Compose v2 is required for the lifecycle E2E" >&2
  exit 1
fi

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
results_directory=$(mktemp -d)
export LKIT_E2E_RESULTS_DIR=$results_directory
export LKIT_E2E_BUCKET="lkit-lifecycle-$(date --utc +%Y%m%d%H%M%S)-$$"
compose=(docker compose --project-directory "$root" --file "$root/scripts/docker-e2e/compose.yaml")

cleanup() {
  "${compose[@]}" down --volumes --remove-orphans >/dev/null 2>&1 || true
  rm -rf "$results_directory"
}
trap cleanup EXIT

# A host reboot or an interrupted previous run can bypass the EXIT trap. Clear
# only this Compose project's containers and volumes before creating fixtures.
"${compose[@]}" down --volumes --remove-orphans >/dev/null 2>&1 || true

"${compose[@]}" build e2e
set +e
"${compose[@]}" up --abort-on-container-exit
compose_status=$?
set -e

result=$results_directory/result.json
if [[ ! -f "$result" ]]; then
  "${compose[@]}" logs >&2 || true
  echo "Docker functional E2E did not produce a result file (compose exit $compose_status)" >&2
  exit 1
fi
cat "$result"
outcome=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["outcome"])' "$result")
if [[ $outcome != passed ]]; then
  if [[ -f "$results_directory/scenario.log" ]]; then
    cat "$results_directory/scenario.log" >&2
  fi
  "${compose[@]}" logs >&2 || true
  exit 1
fi
