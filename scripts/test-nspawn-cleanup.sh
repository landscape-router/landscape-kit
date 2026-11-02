#!/usr/bin/env bash
# Unit tests: all process/service/mount operations are shell-function fakes.
set -euo pipefail
repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
# shellcheck source=scripts/lib/nspawn-cleanup.sh
source "$repo_root/scripts/lib/nspawn-cleanup.sh"
test_root=$(mktemp -d /tmp/lkit-nspawn.cleanup-test.XXXXXX)
trap 'command rm -rf "$test_root"' EXIT
log=$test_root/calls

fail() { echo "nspawn cleanup test: $*" >&2; exit 1; }
reset() {
  root=$test_root/$1
  mkdir -p "$root/rootfs"
  printf 'fixture\n' >"$root/rootfs/file"
  : >"$log"
  alive=true
  waited=false
  probes=0
  mode=graceful
  mounts_mode=clear
  rm_fails=false
  started=true
}

machinectl() { echo "machinectl $*" >>"$log"; }
kill() {
  case $1 in
    -0)
      probes=$((probes + 1))
      if [[ $mode == graceful && $probes -ge 3 ]]; then alive=false; fi
      [[ $alive == true ]]
      ;;
    -TERM)
      echo TERM >>"$log"
      if [[ $mode == term ]]; then alive=false; fi
      ;;
    -KILL)
      echo KILL >>"$log"
      if [[ $mode != stuck ]]; then alive=false; fi
      ;;
    *) fail "unexpected kill arguments: $*" ;;
  esac
}
sleep() { :; }
wait() {
  [[ $alive == false ]] || fail 'wait was called on a still-running fake process'
  waited=true
  echo wait >>"$log"
  return 143 # A terminated child's status must not replace a successful test.
}
findmnt() {
  echo findmnt >>"$log"
  if [[ $mounts_mode == failed ]]; then return 1; fi
  printf '/\n'
  if [[ $mounts_mode == mounted ]]; then printf '%s/rootfs/proc\n' "$root"; fi
}
rm() {
  [[ ${!#} == "$test_root/"* ]] || fail 'attempted deletion outside test territory'
  if [[ $started == true && $waited != true ]]; then fail 'rootfs deletion preceded child reaping'; fi
  echo rm >>"$log"
  if [[ $rm_fails == true ]]; then return 1; fi
  command rm "$@"
}

reset graceful
nspawn_cleanup true fixture-machine 12345 "$root"
[[ ! -e $root ]] || fail 'graceful teardown left its rootfs'
[[ $(<"$log") == $'machinectl terminate fixture-machine\nwait\nfindmnt\nrm' ]] || fail 'incorrect graceful cleanup order'

reset term
mode=term
nspawn_cleanup true fixture-machine 12345 "$root"
grep -qx TERM "$log" || fail 'missing TERM escalation'
if grep -qx KILL "$log"; then fail 'unnecessary KILL after TERM succeeded'; fi
[[ ! -e $root ]] || fail 'TERM teardown left its rootfs'

reset kill
mode='kill'
if nspawn_cleanup true fixture-machine 12345 "$root" 2>/dev/null; then fail 'forced namespace teardown was treated as verified'; fi
grep -qx KILL "$log" || fail 'missing bounded KILL escalation'
[[ -f $root/rootfs/file ]] || fail 'files were deleted after unverified forced teardown'

reset stuck
mode=stuck
if nspawn_cleanup true fixture-machine 12345 "$root" 2>/dev/null; then fail 'a live process was accepted'; fi
[[ -f $root/rootfs/file ]] || fail 'live guest files were removed'
[[ $probes -le 253 ]] || fail 'process exit polling exceeded its bound'

reset mounted
mounts_mode=mounted
if nspawn_cleanup true fixture-machine 12345 "$root" 2>/dev/null; then fail 'live mounts were accepted'; fi
[[ -f $root/rootfs/file ]] || fail 'mounted guest files were removed'

reset inspection-failed
mounts_mode=failed
if nspawn_cleanup true fixture-machine 12345 "$root" 2>/dev/null; then fail 'failed mount inspection was ignored'; fi
[[ -f $root/rootfs/file ]] || fail 'files were removed without mount verification'

reset delete-failed
rm_fails=true
if nspawn_cleanup true fixture-machine 12345 "$root"; then fail 'deletion failure was swallowed'; fi

reset not-started
started=false
nspawn_cleanup false fixture-machine '' "$root"
[[ $(<"$log") == $'findmnt\nrm' ]] || fail 'pre-boot cleanup tried process operations'

reset invalid-pid
if nspawn_cleanup true fixture-machine 1 "$root" 2>/dev/null; then fail 'PID 1 was accepted'; fi
[[ ! -s $log ]] || fail 'invalid PID triggered process or file operations'
if nspawn_cleanup false fixture-machine '' / 2>/dev/null; then fail 'unsafe path was accepted'; fi
[[ ! -s $log ]] || fail 'unsafe path triggered process or file operations'

reset failed-test
started=false
result=0
nspawn_cleanup_status 7 false fixture-machine '' "$root" || result=$?
[[ $result == 7 && ! -e $root ]] || fail 'successful cleanup changed the original test failure'

reset both-failed
started=false
rm_fails=true
result=0
nspawn_cleanup_status 7 false fixture-machine '' "$root" 2>/dev/null || result=$?
[[ $result == 7 ]] || fail 'cleanup failure replaced the original test failure'

reset successful-test-failed-cleanup
started=false
rm_fails=true
result=0
nspawn_cleanup_status 0 false fixture-machine '' "$root" 2>/dev/null || result=$?
[[ $result == 1 ]] || fail 'cleanup failure was hidden after a successful test'

echo 'nspawn cleanup test: passed'
