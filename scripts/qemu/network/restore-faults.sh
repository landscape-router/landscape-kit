#!/usr/bin/env bash
# Source from the QEMU host entry. All service/network actions remain in its VM.
[[ ${BASH_SOURCE[0]} != "$0" ]] || { echo 'Source this helper from the QEMU acceptance entry' >&2; exit 2; }

# Provided by the host entry: journey_guest, journey_api, lan_ssh,
# reboot_restored_vm, powercut_restored_vm, restore_real_backup and VM variables.
wait_for_restore_fault() {
  local scenario=$1
  local stage=$2
  local seconds=$3
  local deadline=$((SECONDS + seconds))
  local next_progress=$((SECONDS + 60))
  while (( SECONDS < deadline )); do
    if lan_ssh "bash /var/lib/lkit-qemu/journey-guest.sh fault-$stage $scenario" >/dev/null 2>&1; then
      journey_guest "fault-$stage $scenario"
      return 0
    fi
    # Variables in this command are intentionally evaluated only in the guest.
    # shellcheck disable=SC2016
    if [[ $stage == pending ]] && lan_ssh 'test -f /var/lib/lkit-qemu/journey/restore-file.exit && test "$(cat /var/lib/lkit-qemu/journey/restore-file.exit)" -ne 0' >/dev/null 2>&1; then
      echo "restore fault $scenario failed before confirmation" >&2
      return 1
    fi
    # shellcheck disable=SC2154
    kill -0 "$active_qemu_pid" >/dev/null 2>&1 || return 1
    if (( SECONDS >= next_progress )); then
      printf 'Waiting for real restore %s %s (%ss remaining)\n' "$scenario" "$stage" "$((deadline - SECONDS))"
      next_progress=$((SECONDS + 60))
    fi
    sleep 3
  done
  # Preserve a concrete assertion failure alongside the normal VM diagnostics.
  journey_guest "fault-$stage $scenario" || true
  echo "restore fault $scenario did not reach $stage" >&2
  return 1
}

run_restore_fault_journey() {
  local scenario wait_seconds remaining
  # Each rollback returns to this same edited state; do not reapply deletes.
  journey_api mutate
  journey_api assert-mutated
  for scenario in explicit timeout reboot powercut; do
    [[ $scenario != explicit ]] || journey_guest runtime-enable
    journey_guest "fault-start $scenario"
    wait_for_restore_fault "$scenario" pending 240
    journey_guest offline
    # Prove the backup was actually applied before rejecting it.
    journey_api assert
    wait_seconds=240
    case "$scenario" in
      explicit)
        journey_guest fault-rollback
        ;;
      timeout)
        remaining=$(lan_ssh 'bash /var/lib/lkit-qemu/journey-guest.sh fault-deadline timeout')
        [[ $remaining =~ ^[0-9]+$ ]] || { echo 'Invalid restore confirmation time remaining' >&2; return 1; }
        wait_seconds=$((remaining + 240))
        echo 'Waiting for the production 600-second confirmation window; transaction deadlines are not modified'
        ;;
      reboot)
        reboot_restored_vm present
        ;;
      powercut)
        # Host hook abruptly kills QEMU and reopens both existing disks with WAN down.
        powercut_restored_vm
        ;;
    esac
    wait_for_restore_fault "$scenario" rolled-back "$wait_seconds"
    [[ $scenario != explicit ]] || journey_guest runtime-restored
    journey_guest offline
    journey_api assert-mutated
    [[ $scenario != explicit ]] || journey_guest runtime-persistent
  done
  journey_guest backup-after-faults
  # shellcheck disable=SC2154
  scp "${ssh_common[@]}" root@192.168.10.1:/var/lib/lkit-qemu/journey/after-faults.lkb \
    "$test_root/journey/after-faults.lkb"
  chmod 0600 "$test_root/journey/after-faults.lkb"
  journey_api verify-mutated-backup
  # Leave the successful baseline for the normal reboot/self-remove acceptance.
  restore_real_backup file
  echo 'PASS: explicit rollback, real expiration, VM reboot and abrupt power loss preserve pre-restore business data'
}
