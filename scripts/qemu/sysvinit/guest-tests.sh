#!/usr/bin/env bash
# Destructive assertions run only inside the disposable SysVinit QEMU guest.
set -euo pipefail
[[ -f /etc/lkit-ci-sysvinit && $(hostname) == lkit-sysvinit-ci && $EUID == 0 ]] || {
  echo 'Requires the disposable SysVinit CI guest' >&2
  exit 2
}
mkdir -p /var/log/lkit-qemu /var/lib/lkit-qemu
results=/var/log/lkit-qemu/results.tsv
stage=${1:?initial, enabled-reboot, or disabled-reboot required}
failed() {
  local status=$?
  printf 'FAIL\t%s\tline %s\n' "$stage" "$1" >>"$results"
  exit "$status"
}
trap 'failed "$LINENO"' ERR

pass() { printf 'PASS\t%s\t%s\n' "$stage" "$1" | tee -a "$results"; }
wait_for() {
  local attempt
  for ((attempt = 0; attempt < 60; attempt++)); do
    if "$@"; then return 0; fi
    sleep 1
  done
  echo "Timed out: $*" >&2
  return 1
}

process_is() {
  local pid=$1 arguments=$2
  [[ $pid =~ ^[1-9][0-9]*$ && -r /proc/$pid/cmdline ]] || return 1
  [[ $(readlink -f "/proc/$pid/exe") == /usr/local/bin/lkit ]] || return 1
  [[ $(tr '\0' ' ' <"/proc/$pid/cmdline") == "$arguments " ]]
}

daemon_ready() {
  local daemon supervisor capability
  daemon=$(cat /root/.lkit/run/lkit.pid 2>/dev/null) || return 1
  supervisor=$(cat /run/lkit.pid 2>/dev/null) || return 1
  process_is "$supervisor" '/usr/local/bin/lkit daemon --supervise' || return 1
  process_is "$daemon" '/usr/local/bin/lkit daemon' || return 1
  [[ $(awk '/^PPid:/ {print $2}' "/proc/$daemon/status") == "$supervisor" ]] || return 1
  for capability in service-repair-v1 network-recovery-v1 operations-v3 asset-repair-v2; do
    [[ $(cat "/root/.lkit/run/$capability.pid" 2>/dev/null) == "$daemon" ]] || return 1
  done
}

no_daemon() {
  # Match executable plus arguments, not stale PID files or the test shell.
  ! pgrep -f '^/usr/local/bin/lkit daemon( --supervise)?$' >/dev/null
}

startup_links() { find /etc/rc?.d -maxdepth 1 -type l -name 'S*lkit.service' -print; }
all_links() { find /etc/rc?.d -maxdepth 1 -type l -name '*lkit.service' -print; }

assert_identity() {
  cat /etc/os-release
  uname -r
  printf 'PID 1: %s\n' "$(readlink -f /proc/1/exe)"
  lkit --version
  [[ $(cat /proc/1/comm) == init ]]
  [[ $(readlink -f /proc/1/exe) == "$(readlink -f /sbin/init)" ]]
  dpkg-query -W -f='${Status}\n' sysvinit-core | grep -Fx 'install ok installed'
  [[ ! -d /run/systemd/system && ! -x /sbin/openrc ]]
  # General host suitability is outside this lifecycle test; assert the backend row.
  lkit --lang en check --color never >"/var/log/lkit-qemu/check-$stage.log" 2>&1 || true
  grep -E '^PASS.*init[.]backend[[:space:]].*SysVinit' "/var/log/lkit-qemu/check-$stage.log"
  pass 'real SysVinit PID 1 and lkit backend selection'
}

assert_new_boot() {
  [[ $(cat /proc/sys/kernel/random/boot_id) != "$(cat /var/lib/lkit-qemu/previous-boot)" ]]
  pass 'guest boot ID changed'
}

remember_boot() { cat /proc/sys/kernel/random/boot_id >/var/lib/lkit-qemu/previous-boot; }

assert_identity
case "$stage" in
  initial)
    no_daemon
    lkit self install --flare-psk-file /root/ci-flare.psk
    wait_for daemon_ready
    [[ -x /etc/init.d/lkit.service ]]
    [[ $(readlink -f /etc/init.d/lkit.service) == /usr/local/lib/lkit/lkit.service ]]
    [[ -n $(startup_links) ]]
    pass 'self install registers, enables and starts the daemon with capabilities'

    old_daemon=$(cat /root/.lkit/run/lkit.pid)
    old_supervisor=$(cat /run/lkit.pid)
    kill -KILL "$old_daemon"
    restarted() { daemon_ready && [[ $(cat /root/.lkit/run/lkit.pid) != "$old_daemon" ]]; }
    wait_for restarted
    [[ $(cat /run/lkit.pid) == "$old_supervisor" ]]
    pass 'SIGKILL of daemon child is recovered by the same supervisor'

    /etc/init.d/lkit.service stop
    wait_for no_daemon
    # Observe longer than the supervisor maximum backoff, checking every second.
    for _ in $(seq 1 12); do sleep 1; no_daemon; done
    pass 'explicit stop leaves no daemon or supervisor and does not respawn'

    /etc/init.d/lkit.service start
    wait_for daemon_ready
    remember_boot
    pass 'enabled service is ready for reboot'
    ;;
  enabled-reboot)
    assert_new_boot
    wait_for daemon_ready
    [[ -n $(startup_links) ]]
    pass 'enabled service starts automatically after reboot'

    update-rc.d lkit.service disable
    [[ -z $(startup_links) && -n $(all_links) ]]
    /etc/init.d/lkit.service stop
    wait_for no_daemon
    remember_boot
    pass 'disable removes start links and retains stop links'
    ;;
  disabled-reboot)
    assert_new_boot
    [[ -z $(startup_links) ]]
    for _ in $(seq 1 12); do sleep 1; no_daemon; done
    pass 'disabled service stays stopped after reboot'

    /etc/init.d/lkit.service start
    wait_for daemon_ready
    [[ -z $(startup_links) ]]
    lkit self remove
    wait_for no_daemon
    [[ ! -e /etc/init.d/lkit.service && ! -L /etc/init.d/lkit.service ]]
    [[ ! -e /usr/local/lib/lkit/lkit.service && -z $(all_links) ]]
    [[ -x /usr/local/bin/lkit ]]
    lkit self remove
    no_daemon
    pass 'self remove stops the disabled service and removes all registration links; retry is safe'
    ;;
  *) echo "Unknown stage: $stage" >&2; exit 2 ;;
esac
