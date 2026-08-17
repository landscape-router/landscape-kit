#!/usr/bin/env bash
set -uo pipefail

result=${LKIT_E2E_RESULTS:-/results/result.json}
mkdir -p "$(dirname "$result")"
started_at=$(date --utc +%Y-%m-%dT%H:%M:%SZ)

scenario_log=$(dirname "$result")/scenario.log
set +e
/usr/local/lib/lkit-e2e/run-scenarios.sh 2>&1 | tee "$scenario_log"
status=${PIPESTATUS[0]}
set -e

if [[ $status -eq 0 ]]; then
  outcome=passed
else
  outcome=failed
fi
python3 - "$result" "$outcome" "$status" "$started_at" <<'PY'
import json
import os
import sys

path, outcome, status, started_at = sys.argv[1:]
temporary = path + ".tmp"
with open(temporary, "w", encoding="utf-8") as stream:
    json.dump(
        {
            "schema_version": 1,
            "outcome": outcome,
            "exit_code": int(status),
            "started_at": started_at,
        },
        stream,
        indent=2,
    )
    stream.write("\n")
os.replace(temporary, path)
PY
sync
exit "$status"
