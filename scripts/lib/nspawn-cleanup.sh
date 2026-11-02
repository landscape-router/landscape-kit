#!/usr/bin/env bash
# Teardown only the nspawn instance and temporary tree owned by the caller.
# Tests replace machinectl/kill/wait/sleep/findmnt; never boot a real container.

# Return the test's failure status unchanged; cleanup failures turn an otherwise
# successful test into a failure rather than hiding teardown problems.
nspawn_cleanup_status() {
  local status=$1
  shift
  if ! nspawn_cleanup "$@"; then
    echo "nspawn cleanup failed; temporary files retained at ${4:-unknown}" >&2
    if [[ $status == 0 ]]; then status=1; fi
  fi
  return "$status"
}

nspawn_wait_for_exit() {
  local pid=$1 attempts=$2 attempt
  for ((attempt = 0; attempt < attempts; attempt++)); do
    if ! kill -0 "$pid" 2>/dev/null; then
      return 0
    fi
    sleep 0.1
  done
  ! kill -0 "$pid" 2>/dev/null
}

nspawn_cleanup() {
  local started=$1 machine=$2 pid=$3 root=$4 mounts target
  case "$root" in
    /tmp/lkit-nspawn.* | /var/tmp/lkit-nspawn.*) ;;
    *) echo "Refusing to remove unexpected nspawn temporary path: $root" >&2; return 1 ;;
  esac
  if [[ $started == true ]]; then
    if [[ ! $pid =~ ^[1-9][0-9]*$ || $pid == 1 ]]; then
      echo 'Cannot clean up nspawn: invalid owned process ID' >&2
      return 1
    fi
    machinectl terminate "$machine" >/dev/null 2>&1 || true
    # terminate is asynchronous. Wait for PID 1 shutdown, namespace teardown and
    # the parent nspawn process to exit before removing the guest filesystem.
    if ! nspawn_wait_for_exit "$pid" 100; then
      kill -TERM "$pid" 2>/dev/null || true
      if ! nspawn_wait_for_exit "$pid" 100; then
        kill -KILL "$pid" 2>/dev/null || true
        if nspawn_wait_for_exit "$pid" 50; then
          wait "$pid" 2>/dev/null || true
        fi
        # SIGKILL bypasses nspawn's orderly namespace teardown. Even if the
        # parent exited, do not assume that guest writers and mounts are gone.
        echo "nspawn required forced termination; preserving $root for inspection" >&2
        return 1
      fi
    fi
    # A deliberately terminated container can exit nonzero. Its exit code is
    # not the test result; waiting here reaps the owned child process.
    wait "$pid" 2>/dev/null || true
  fi

  mounts=$(findmnt --raw --noheadings --output TARGET) || {
    echo "Cannot inspect remaining mounts; preserving $root" >&2
    return 1
  }
  while IFS= read -r target; do
    if [[ $target == "$root" || $target == "$root/"* ]]; then
      echo "Mount still present at $target; preserving $root" >&2
      return 1
    fi
  done <<<"$mounts"
  rm -rf --one-file-system -- "$root"
}
