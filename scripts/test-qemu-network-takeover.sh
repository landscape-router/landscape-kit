#!/usr/bin/env bash
set -euo pipefail

if [[ $(uname -s):$(uname -m) != Linux:x86_64 ]]; then
  echo "QEMU network takeover requires Linux x86_64" >&2
  exit 2
fi
if [[ $EUID -ne 0 ]]; then
  echo "QEMU network takeover must run as root" >&2
  exit 2
fi
if [[ ! -c /dev/kvm || ! -r /dev/kvm || ! -w /dev/kvm ]]; then
  echo "/dev/kvm is unavailable; this test does not fall back to TCG" >&2
  exit 2
fi

required_commands=(
  curl dpkg-deb ip jq mke2fs mmdebstrap python3 qemu-img qemu-system-x86_64
  scp sha256sum ssh ssh-keygen timeout
)
for command_name in "${required_commands[@]}"; do
  command -v "$command_name" >/dev/null 2>&1 || {
    echo "$command_name is required for the QEMU network takeover test" >&2
    exit 2
  }
done
qemu-system-x86_64 -accel help | grep -qw kvm || {
  echo "qemu-system-x86_64 does not provide the KVM accelerator" >&2
  exit 2
}

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
test_root=$(mktemp -d /var/tmp/lkit-qemu-network.XXXXXX)
artifact_dir=${LKIT_QEMU_ARTIFACT_DIR:-$test_root/artifacts}
mkdir -p "$artifact_dir"
active_qemu_pid=
active_tap=
active_qmp=
active_disk=
active_installation_disk=
active_scenario=
active_boot=0
active_wan_up=true
landscape_version=$(jq -r .version "$repo_root/scripts/qemu/network/landscape-release.json")
cp "$repo_root/scripts/qemu/network/landscape-release.json" "$artifact_dir/landscape-release.json"

cleanup_vm() {
  if [[ -n $active_qemu_pid ]]; then
    kill "$active_qemu_pid" >/dev/null 2>&1 || true
    wait "$active_qemu_pid" >/dev/null 2>&1 || true
    active_qemu_pid=
  fi
  if [[ -n $active_tap ]]; then
    ip link delete "$active_tap" >/dev/null 2>&1 || true
    active_tap=
  fi
}

cleanup() {
  local result=$?
  if declare -F close_flare_ssh >/dev/null; then close_flare_ssh; fi
  if [[ $result -ne 0 && -n $active_qemu_pid ]] && declare -F collect_guest_diagnostics >/dev/null; then
    collect_guest_diagnostics failure || true
  fi
  cleanup_vm
  rm -rf "$test_root"
}
trap cleanup EXIT

if [[ -n ${LKIT_QEMU_PREBUILT:-} ]]; then
  lkit_binary=$(realpath "$LKIT_QEMU_PREBUILT")
else
  qemu_target=$test_root/target
  cd "$repo_root"
  CARGO_TARGET_DIR="$qemu_target" cargo build --locked --release -p lkit-cli --bin lkit
  lkit_binary=$qemu_target/release/lkit
fi
[[ -x $lkit_binary ]] || {
  echo "missing lkit executable $lkit_binary" >&2
  exit 2
}

ssh_key=$test_root/id_ed25519
ssh-keygen -q -t ed25519 -N '' -f "$ssh_key"
public_key=$(<"$ssh_key.pub")
rootfs=$test_root/rootfs
keyring_deb=$test_root/debian-archive-keyring.deb
keyring_root=$test_root/debian-keyring
curl --fail --location --silent --show-error \
  https://deb.debian.org/debian/pool/main/d/debian-archive-keyring/debian-archive-keyring_2025.1_all.deb \
  --output "$keyring_deb"
printf '%s  %s\n' \
  9ea7778e443144ca490668737a8ab22dd3e748bb99e805e22ec055abeb3c7fac \
  "$keyring_deb" | sha256sum --check --status
dpkg-deb --extract "$keyring_deb" "$keyring_root"
archive_keyring=$keyring_root/usr/share/keyrings/debian-archive-keyring.gpg
[[ -f $archive_keyring ]] || {
  echo "verified Debian archive keyring package did not contain its keyring" >&2
  exit 1
}

mmdebstrap \
  --variant=minbase \
  --architectures=amd64 \
  --keyring="$archive_keyring" \
  --include=systemd,systemd-sysv,dbus,linux-image-cloud-amd64,openssh-server,network-manager,firewalld,systemd-resolved,ppp,iproute2,nftables,curl,ca-certificates,jq,procps,kmod,python3 \
  trixie "$rootfs" https://deb.debian.org/debian

install -D -m 0755 "$lkit_binary" "$rootfs/usr/local/bin/lkit"
install -d -m 0700 "$rootfs/root/.ssh"
printf '%s\n' "$public_key" >"$rootfs/root/.ssh/authorized_keys"
chmod 0600 "$rootfs/root/.ssh/authorized_keys"
install -d -m 0755 "$rootfs/etc/ssh/sshd_config.d"
printf '%s\n' 'PermitRootLogin prohibit-password' >"$rootfs/etc/ssh/sshd_config.d/90-lkit-qemu.conf"
printf '%s\n' 'lkit-qemu' >"$rootfs/etc/hostname"
printf '%s\n' '/dev/vda / ext4 defaults 0 1' '/dev/vdb /srv ext4 defaults 0 2' >"$rootfs/etc/fstab"
mkdir -p "$rootfs/srv"
printf '%s\n' 'Disposable QEMU network acceptance guest' >"$rootfs/etc/lkit-qemu-network"
install -d -m 0700 "$rootfs/var/lib/lkit-qemu"
install -m 0755 "$lkit_binary" "$rootfs/var/lib/lkit-qemu/candidate-lkit"
sha256sum "$lkit_binary" | cut -d ' ' -f 1 >"$rootfs/var/lib/lkit-qemu/candidate-lkit.sha256"
upgrade_pin=$repo_root/scripts/qemu/network/lkit-upgrade-releases.json
legacy_tag=$(jq -r '.source.tag' "$upgrade_pin")
legacy_asset=$(jq -r '.asset' "$upgrade_pin")
legacy_sha256=$(jq -r '.source.sha256' "$upgrade_pin")
curl --fail --location --silent --show-error --retry 3 \
  "https://github.com/landscape-router/landscape-kit/releases/download/$legacy_tag/$legacy_asset" \
  --output "$rootfs/var/lib/lkit-qemu/legacy-lkit-rc3"
printf '%s  %s\n' "$legacy_sha256" "$rootfs/var/lib/lkit-qemu/legacy-lkit-rc3" | sha256sum --check --status
chmod 0755 "$rootfs/var/lib/lkit-qemu/legacy-lkit-rc3"
cp "$upgrade_pin" "$artifact_dir/lkit-upgrade-releases.json"
printf '%s\n' 'Secret123' >"$rootfs/var/lib/lkit-qemu/password"
chmod 0600 "$rootfs/var/lib/lkit-qemu/password"
printf '%s\n' 'Secret123' >"$test_root/journey-password"
chmod 0600 "$test_root/journey-password"
if [[ -n ${GITHUB_TOKEN:-} ]]; then
  # The CLI and resident daemon both resolve official release metadata.
  # Keep CI credentials in the disposable disk, outside logs and artifacts.
  (umask 077; printf 'GITHUB_TOKEN=%s\n' "$GITHUB_TOKEN" >"$rootfs/var/lib/lkit-qemu/github.env")
  install -d -m 0755 "$rootfs/etc/systemd/system/lkit.service.d"
  cat >"$rootfs/etc/systemd/system/lkit.service.d/github.conf" <<'UNIT'
[Service]
EnvironmentFile=/var/lib/lkit-qemu/github.env
UNIT
fi

install -d -m 0700 "$rootfs/etc/NetworkManager/system-connections"
cat >"$rootfs/etc/NetworkManager/system-connections/wan.nmconnection" <<'EOF'
[connection]
id=lkit-wan
uuid=1c776a18-1caf-4ac8-8f46-5573fb8db501
type=ethernet
autoconnect=true

[ethernet]
mac-address=52:54:00:12:34:01

[ipv4]
method=auto

[ipv6]
method=link-local
EOF
chmod 0600 "$rootfs/etc/NetworkManager/system-connections/wan.nmconnection"
cat >"$rootfs/etc/NetworkManager/system-connections/lan.nmconnection" <<'EOF'
[connection]
id=lkit-lan-unconfigured
uuid=bc58c211-eb08-47b3-bd07-59a9a55bf293
type=ethernet
autoconnect=false

[ethernet]
mac-address=52:54:00:12:34:02

[ipv4]
method=disabled

[ipv6]
method=disabled
EOF
chmod 0600 "$rootfs/etc/NetworkManager/system-connections/lan.nmconnection"

systemctl --root="$rootfs" disable systemd-networkd.service systemd-networkd.socket >/dev/null 2>&1 || true
systemctl --root="$rootfs" enable NetworkManager.service firewalld.service systemd-resolved.service ssh.service >/dev/null
rm -f "$rootfs/etc/resolv.conf"
ln -s ../run/systemd/resolve/stub-resolv.conf "$rootfs/etc/resolv.conf"

kernel=$(find "$rootfs/boot" -maxdepth 1 -type f -name 'vmlinuz-*' | sort | tail -1)
initrd=$(find "$rootfs/boot" -maxdepth 1 -type f -name 'initrd.img-*' | sort | tail -1)
[[ -n $kernel && -n $initrd ]] || {
  echo "mmdebstrap rootfs does not contain a bootable kernel and initrd" >&2
  exit 1
}

base_disk=$test_root/base.ext4
truncate -s 12G "$base_disk"
mke2fs -q -t ext4 -F -d "$rootfs" "$base_disk"

ssh_common=(
  -i "$ssh_key"
  -o BatchMode=yes
  -o ConnectTimeout=5
  -o ServerAliveInterval=2
  -o ServerAliveCountMax=3
  -o StrictHostKeyChecking=no
  -o UserKnownHostsFile=/dev/null
)

next_port() {
  python3 - <<'PY'
import socket
with socket.socket() as sock:
    sock.bind(("127.0.0.1", 0))
    print(sock.getsockname()[1])
PY
}

wait_for_ssh() {
  local mode=$1
  local deadline=$((SECONDS + 240))
  while (( SECONDS < deadline )); do
    if [[ $mode == wan ]]; then
      ssh "${ssh_common[@]}" -p "$active_ssh_port" root@127.0.0.1 true >/dev/null 2>&1 && return 0
    else
      ssh "${ssh_common[@]}" root@192.168.10.1 true >/dev/null 2>&1 && return 0
    fi
    kill -0 "$active_qemu_pid" >/dev/null 2>&1 || return 1
    sleep 2
  done
  return 1
}

wait_for_takeover_ready() {
  local deadline=$((SECONDS + 240))
  while ((SECONDS < deadline)); do
    if lan_ssh "jq -e '.phase == \"awaiting_network_confirmation\"' /root/.lkit/transactions/*.json" \
      >/dev/null 2>&1; then
      return 0
    fi
    if lan_ssh "jq -e '.phase == \"failed\" or .phase == \"rolled_back\"' /root/.lkit/transactions/*.json" \
      >/dev/null 2>&1; then
      return 1
    fi
    kill -0 "$active_qemu_pid" >/dev/null 2>&1 || return 1
    sleep 2
  done
  return 1
}

wan_ssh() {
  ssh "${ssh_common[@]}" -p "$active_ssh_port" root@127.0.0.1 "$@"
}

lan_ssh() {
  # Arguments intentionally contain a remote shell command.
  # shellcheck disable=SC2029
  ssh "${ssh_common[@]}" root@192.168.10.1 "$@"
}

collect_guest_diagnostics() {
  local scenario=$1
  local output=$artifact_dir/$scenario-guest-diagnostics.log
  # This script expands variables only inside the guest.
  # shellcheck disable=SC2016
  local diagnostic_command='set +e
echo "== recovery journal =="
journalctl -b --no-pager -u lkit.service -u NetworkManager.service -u lkit-qemu-recovery-check.service
echo "== NetworkManager connections =="
nmcli connection show --active
echo "== landscape journal =="
journalctl -b --no-pager -u landscape-router.service
echo "== landscape files =="
find /srv/landscape -maxdepth 4 -type f -printf "%p\n" | sort
echo "== landscape logs =="
find /srv/landscape/data/logs -maxdepth 1 -type f -exec sh -c '\''for file do echo "--- $file"; tail -n 300 "$file"; done'\'' sh {} +
echo "== restore command results =="
for file in /var/lib/lkit-qemu/journey/restore-*.log; do test -f "$file" && { echo "--- $file"; tail -n 100 "$file"; }; done
echo "== transactions =="
for file in /root/.lkit/transactions/*.json; do test -f "$file" && { echo "--- $file"; cat "$file"; }; done
echo "== service state =="
systemctl --no-pager --full status landscape-router.service NetworkManager.service firewalld.service systemd-resolved.service
echo "== network state =="
ip -details link show
ip -4 address show
ip -4 route show table all
echo "== kernel state =="
uname -a
mount
sysctl net.ipv4.ip_forward kernel.unprivileged_bpf_disabled'

  if lan_ssh "$diagnostic_command" >"$output" 2>&1; then
    return
  fi
  wan_ssh "$diagnostic_command" >"$output" 2>&1 || true
}

assert_lan() {
  local scenario=$1
  local description=$2
  local command=$3
  if lan_ssh "$command"; then
    return
  fi
  collect_guest_diagnostics "$scenario"
  echo "$scenario assertion failed: $description" >&2
  return 1
}

assert_confirmed_wan_plan() {
  local scenario=$1
  local wan
  local wan_mode
  local installation='map(select(.operation == "install" and .network_takeover != null)) | last'
  if ! wan=$(lan_ssh "jq -sr '$installation | .network_takeover.plan.mode.wan' /root/.lkit/transactions/*.json"); then
    collect_guest_diagnostics "$scenario"
    echo "$scenario could not read the confirmed WAN interface" >&2
    return 1
  fi
  if ! wan_mode=$(lan_ssh "jq -sr '$installation | .network_takeover.plan.mode.wan_ipv4.mode // \"none\"' /root/.lkit/transactions/*.json"); then
    collect_guest_diagnostics "$scenario"
    echo "$scenario could not read the confirmed WAN IPv4 mode" >&2
    return 1
  fi

  case "$wan_mode" in
    static)
      local address
      local prefix
      address=$(lan_ssh "jq -sr '$installation | .network_takeover.plan.mode.wan_ipv4.address.address' /root/.lkit/transactions/*.json") || {
        collect_guest_diagnostics "$scenario"
        echo "$scenario could not read the confirmed static WAN address" >&2
        return 1
      }
      prefix=$(lan_ssh "jq -sr '$installation | .network_takeover.plan.mode.wan_ipv4.address.prefix' /root/.lkit/transactions/*.json") || {
        collect_guest_diagnostics "$scenario"
        echo "$scenario could not read the confirmed static WAN prefix" >&2
        return 1
      }
      assert_lan "$scenario" "confirmed WAN keeps its planned static IPv4 address" \
        "ip -4 -o address show dev '$wan' | grep -Fq ' inet ${address}/${prefix} '"
      ;;
    dhcp)
      assert_lan "$scenario" "confirmed WAN has a DHCP IPv4 lease" \
        "ip -4 -o address show dev '$wan' | grep -q ' inet '"
      ;;
    *)
      collect_guest_diagnostics "$scenario"
      echo "$scenario has unsupported confirmed WAN IPv4 mode: $wan_mode" >&2
      return 1
      ;;
  esac
}

start_qemu() {
  active_boot=$((active_boot + 1))
  active_qmp=$test_root/$active_scenario-$active_boot.qmp
  qemu-system-x86_64 \
    -name "lkit-$active_scenario" \
    -machine q35,accel=kvm \
    -cpu host \
    -smp 2 \
    -m 4096 \
    -kernel "$kernel" \
    -initrd "$initrd" \
    -append 'root=/dev/vda rw console=ttyS0 systemd.log_target=console' \
    -drive "file=$active_disk,format=raw,if=virtio,cache=writeback" \
    -drive "file=$active_installation_disk,format=raw,if=virtio,cache=writeback" \
    -netdev "user,id=wan,ipv4=on,ipv6=on,hostfwd=tcp:127.0.0.1:${active_ssh_port}-:22" \
    -device virtio-net-pci,netdev=wan,mac=52:54:00:12:34:01 \
    -netdev "tap,id=lan,ifname=$active_tap,script=no,downscript=no" \
    -device virtio-net-pci,netdev=lan,mac=52:54:00:12:34:02 \
    -display none \
    -monitor none \
    -S \
    -qmp "unix:$active_qmp,server=on,wait=off" \
    -serial stdio \
    >"$artifact_dir/$active_scenario-boot-$active_boot-serial.log" 2>&1 &
  active_qemu_pid=$!
  # Configure carrier while guest CPUs are paused. A cold restart must not
  # briefly reconnect WAN during an offline recovery acceptance scenario.
  python3 - "$active_qmp" "$active_wan_up" <<'PY'
import json
import socket
import sys
import time
deadline = time.monotonic() + 10
with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as monitor:
    monitor.settimeout(10)
    while True:
        try:
            monitor.connect(sys.argv[1])
            break
        except (FileNotFoundError, ConnectionRefusedError):
            if time.monotonic() >= deadline:
                raise
            time.sleep(0.1)
    stream = monitor.makefile('rwb', buffering=0)
    assert 'QMP' in json.loads(stream.readline())
    for command in [
        {'execute': 'qmp_capabilities'},
        {'execute': 'set_link', 'arguments': {'name': 'wan', 'up': sys.argv[2] == 'true'}},
        {'execute': 'cont'},
    ]:
        stream.write(json.dumps(command).encode() + b'\n')
        while True:
            reply = json.loads(stream.readline())
            if 'event' not in reply:
                assert 'return' in reply and 'error' not in reply, reply
                break
PY
}

boot_vm() {
  local scenario=$1
  active_scenario=$scenario
  active_boot=0
  active_wan_up=true
  active_disk=$test_root/$scenario.ext4
  active_installation_disk=$test_root/$scenario-installation.ext4
  cp --reflink=auto --sparse=always "$base_disk" "$active_disk"
  truncate -s 4G "$active_installation_disk"
  mke2fs -q -t ext4 -F "$active_installation_disk"
  active_tap=lkitlan0
  ip tuntap add dev "$active_tap" mode tap
  ip address add 192.168.10.2/24 dev "$active_tap"
  ip link set "$active_tap" up
  active_ssh_port=$(next_port)
  start_qemu
  wait_for_ssh wan || {
    echo "$scenario VM did not expose SSH through its WAN" >&2
    return 1
  }
  # Record the actual distribution, PID 1 and lkit backend for this VM.
  # shellcheck disable=SC2016
  wan_ssh 'set -eu
cat /etc/os-release
uname -r
readlink /proc/1/exe
test "$(cat /proc/1/comm)" = systemd
systemctl --version
/usr/local/bin/lkit --version
/usr/local/bin/lkit --lang en check > /var/lib/lkit-qemu/init-check.log 2>&1 || true
grep -E "^PASS.*init[.]backend.*systemd" /var/lib/lkit-qemu/init-check.log
' >"$artifact_dir/$scenario-environment.log" 2>&1
  wan_ssh 'systemctl set-environment RUST_BACKTRACE=1 LANDSCAPE_LOG_TERMINAL=true'
  wan_ssh 'systemctl is-active --quiet NetworkManager && systemctl is-active --quiet firewalld && systemctl is-active --quiet systemd-resolved && systemctl is-active --quiet ssh' >/dev/null
  tar -C "$repo_root/scripts/qemu/network" -cf - . | wan_ssh 'tar -C /var/lib/lkit-qemu -xf - && bash /var/lib/lkit-qemu/setup.sh'
}

interface_selection() {
  local wan_iface
  wan_iface=$(wan_ssh "ip -j -4 route show default | jq -r '.[0].dev'")
  mapfile -t physical_ifaces < <(
    wan_ssh "for path in /sys/class/net/*; do name=\${path##*/}; [[ \$name != lo && -e \$path/device && \$(cat \$path/type) = 1 ]] && echo \$name; done | sort"
  )
  [[ ${#physical_ifaces[@]} -eq 2 ]] || {
    echo "expected two physical Ethernet interfaces, found: ${physical_ifaces[*]}" >&2
    return 1
  }
  for index in "${!physical_ifaces[@]}"; do
    if [[ ${physical_ifaces[$index]} == "$wan_iface" ]]; then
      printf '%s' "$((index + 1))"
      return 0
    fi
  done
  echo "default-route interface $wan_iface is not in the physical interface list" >&2
  return 1
}

start_takeover() {
  local scenario=$1
  local wan_index
  wan_index=$(interface_selection)
  set +e
  wan_ssh 'systemctl is-active --quiet lkit.service || /usr/local/bin/lkit self install' \
    || {
      echo "failed to ensure the resident lkit daemon" >&2
      return 1
    }
  printf '%s\n1\n\n\n\n' "$wan_index" | timeout 900 ssh "${ssh_common[@]}" \
    -tt -p "$active_ssh_port" root@127.0.0.1 \
    bash /var/lib/lkit-qemu/install.sh --takeover-network \
    --install-dir /srv/landscape \
    --admin-user admin --password-file /var/lib/lkit-qemu/password \
    --version "$landscape_version" \
    >"$artifact_dir/$scenario-install.log" 2>&1
  local install_status=$?
  set -e
  if [[ $install_status -ne 0 && $install_status -ne 124 && $install_status -ne 255 ]]; then
    wait_for_ssh wan || true
    collect_guest_diagnostics "$scenario"
    echo "takeover command failed before the expected SSH disconnect (status $install_status)" >&2
    return 1
  fi
  wait_for_ssh lan || {
    collect_guest_diagnostics "$scenario"
    echo "$scenario VM did not expose SSH on br_lan" >&2
    return 1
  }
  wait_for_takeover_ready || {
    collect_guest_diagnostics "$scenario"
    echo "$scenario takeover did not reach network confirmation" >&2
    return 1
  }
  assert_lan "$scenario" "br_lan management address" \
    "ip -4 -o address show dev br_lan | grep -q '192.168.10.1/24'"
  assert_lan "$scenario" "one physical LAN member in br_lan" \
    "ip -j link show master br_lan | jq -e 'length == 1'" >/dev/null
  assert_lan "$scenario" "WAN keeps its inherited IPv4 address before confirmation" \
    "ip -4 -o address show dev \$(jq -r '.network_takeover.plan.mode.wan' /root/.lkit/transactions/*.json) | grep -q ' inet '"
  for unit in NetworkManager.service firewalld.service systemd-resolved.service; do
    assert_lan "$scenario" "$unit is masked" \
      "test \"\$(systemctl is-enabled $unit || true)\" = masked"
    assert_lan "$scenario" "$unit is inactive" \
      "test \"\$(systemctl is-active $unit || true)\" = inactive"
  done
}

journey_api() {
  python3 "$repo_root/scripts/qemu/network/journey.py" "$1" \
    --directory "$test_root/journey" --password-file "$test_root/journey-password" \
    --qmp "$active_qmp" | tee -a "$artifact_dir/journey-assertions.log"
  case "$1" in
    link-up) active_wan_up=true ;;
    link-down) active_wan_up=false ;;
  esac
}

journey_guest() {
  lan_ssh "bash /var/lib/lkit-qemu/journey-guest.sh $1" \
    | tee -a "$artifact_dir/journey-assertions.log"
}

restore_real_backup() {
  local mode=$1
  journey_guest "restore-$mode"
  local deadline=$((SECONDS + 240))
  local pending=false
  while (( SECONDS < deadline )); do
    if lan_ssh "bash /var/lib/lkit-qemu/journey-guest.sh pending-$mode" >/dev/null 2>&1; then
      pending=true
      break
    fi
    if lan_ssh "test -f /var/lib/lkit-qemu/journey/restore-$mode.exit && test \"\$(cat /var/lib/lkit-qemu/journey/restore-$mode.exit)\" -ne 0" >/dev/null 2>&1; then
      echo "restore by $mode failed before external confirmation" >&2
      return 1
    fi
    kill -0 "$active_qemu_pid" >/dev/null 2>&1 || return 1
    sleep 2
  done
  [[ $pending == true ]] || { echo "restore by $mode never reached confirmation" >&2; return 1; }
  journey_guest "pending-$mode"
  journey_guest offline
  # These are independent LAN-client assertions while the new state is uncommitted.
  journey_api assert
  lan_ssh '/usr/local/bin/lkit network confirm' \
    | tee -a "$artifact_dir/journey-assertions.log"
  journey_guest committed
}

reboot_restored_vm() {
  local daemon=$1
  local readiness='systemctl is-active --quiet landscape-router.service'
  [[ $daemon != present ]] || readiness+=' && systemctl is-active --quiet lkit.service'
  local previous_boot current_boot
  previous_boot=$(lan_ssh 'cat /proc/sys/kernel/random/boot_id')
  lan_ssh 'systemctl reboot' >/dev/null 2>&1 || true
  local deadline=$((SECONDS + 240))
  local rebooted=false
  while (( SECONDS < deadline )); do
    if current_boot=$(lan_ssh 'cat /proc/sys/kernel/random/boot_id' 2>/dev/null) && [[ $current_boot != "$previous_boot" ]]; then
      if lan_ssh "$readiness" >/dev/null 2>&1; then
        rebooted=true
        break
      fi
    fi
    kill -0 "$active_qemu_pid" >/dev/null 2>&1 || return 1
    sleep 2
  done
  [[ $rebooted == true ]] || { echo "restored VM did not return after a real reboot (daemon $daemon)" >&2; return 1; }
}

powercut_restored_vm() {
  local previous_boot current_boot
  previous_boot=$(lan_ssh 'cat /proc/sys/kernel/random/boot_id')
  # No guest shutdown or sync: abandon the running kernel and reuse both disks.
  kill -KILL "$active_qemu_pid"
  wait "$active_qemu_pid" 2>/dev/null || true
  active_qemu_pid=
  start_qemu
  wait_for_ssh lan
  local deadline=$((SECONDS + 240))
  while (( SECONDS < deadline )); do
    if current_boot=$(lan_ssh 'cat /proc/sys/kernel/random/boot_id' 2>/dev/null) && \
      [[ $current_boot != "$previous_boot" ]] && \
      lan_ssh 'systemctl is-active --quiet landscape-router.service && systemctl is-active --quiet lkit.service' >/dev/null 2>&1; then
      printf '%s\n' 'PASS: abruptly terminated VM cold-boots from the same system and installation disks' \
        | tee -a "$artifact_dir/journey-assertions.log"
      return
    fi
    kill -0 "$active_qemu_pid" >/dev/null 2>&1 || return 1
    sleep 2
  done
  echo 'restored VM did not recover after abrupt power loss' >&2
  return 1
}

# shellcheck source=scripts/qemu/network/restore-faults.sh
source "$repo_root/scripts/qemu/network/restore-faults.sh"

upgrade_guest_async() {
  local action=$1
  lan_ssh "nohup python3 /var/lib/lkit-qemu/upgrade-guest.py $action </dev/null >/var/lib/lkit-qemu/upgrade-$action.log 2>&1 &"
  local deadline=$((SECONDS + 360))
  while (( SECONDS < deadline )); do
    if lan_ssh "test -f /var/lib/lkit-qemu/upgrade/$action.exit" >/dev/null 2>&1; then
      lan_ssh "cat /var/lib/lkit-qemu/upgrade-$action.log" | tee -a "$artifact_dir/upgrade-assertions.log"
      lan_ssh "test \"\$(cat /var/lib/lkit-qemu/upgrade/$action.exit)\" = 0"
      return
    fi
    kill -0 "$active_qemu_pid" >/dev/null 2>&1 || return 1
    sleep 2
  done
  lan_ssh "cat /var/lib/lkit-qemu/upgrade-$action.log" | tee -a "$artifact_dir/upgrade-assertions.log" || true
  echo "legacy upgrade action $action did not complete" >&2
  return 1
}

run_real_backup_journey() {
  journey_guest identity
  journey_api seed
  journey_guest backup
  scp "${ssh_common[@]}" root@192.168.10.1:/var/lib/lkit-qemu/journey/original.lkb \
    "$test_root/journey/original.lkb"
  chmod 0600 "$test_root/journey/original.lkb"
  journey_api verify-backup
  journey_api mutate
  # Physical WAN disconnection remains effective even when restore rewrites routes.
  journey_api link-down
  journey_guest offline
  restore_real_backup id
  journey_guest remove-local-backup
  journey_api mutate
  restore_real_backup file
  run_restore_fault_journey
  journey_api link-up
  assert_confirmed_wan_plan restored

  reboot_restored_vm present
  journey_api assert
  journey_guest committed
  assert_confirmed_wan_plan rebooted
  printf '%s\n' 'PASS: restored official Landscape configuration and external LAN service survive a full VM reboot' \
    | tee -a "$artifact_dir/journey-assertions.log"

  journey_guest remove-daemon
  journey_api assert
  reboot_restored_vm absent
  journey_guest daemon-absent
  journey_api assert
  assert_confirmed_wan_plan without-daemon
  printf '%s\n' 'PASS: committed Landscape boots from its separate filesystem without the lkit daemon and retains real LAN service' \
    | tee -a "$artifact_dir/journey-assertions.log"
}

if [[ ${1:-} == --flare-rollback || ${1:-} == --flare-password ]]; then
  lflare_binary=${LKIT_QEMU_LFLARE:?provide the lflare binary built from this checkout}
  # shellcheck source=scripts/qemu/network/flare-rescue.sh
  source "$repo_root/scripts/qemu/network/flare-rescue.sh"
  case $1 in
    --flare-rollback) run_flare_rollback ;;
    --flare-password) run_flare_password ;;
  esac
  exit 0
fi

boot_vm reboot-rollback
start_takeover reboot-rollback
lan_ssh 'bash -s' >/dev/null <<'GUEST'
jq -e '[.network_takeover.network_manager[] | select(.uuid == "1c776a18-1caf-4ac8-8f46-5573fb8db501" and .ipv4 and .gateway4)] | length == 1' /root/.lkit/transactions/*.json
GUEST
lan_ssh 'mv /etc/NetworkManager/system-connections/wan.nmconnection /var/lib/lkit-qemu/wan.saved; touch /var/lib/lkit-qemu/force-ra; systemctl enable lkit-qemu-recovery-check.service'
lan_ssh 'systemctl reboot' >/dev/null 2>&1 || true
rollback_ready=false
rollback_deadline=$((SECONDS + 300))
while (( SECONDS < rollback_deadline )); do
  if wan_ssh \
    "jq -e '.phase == \"rolled_back\"' /root/.lkit/transactions/*.json >/dev/null && systemctl is-active --quiet NetworkManager && systemctl is-active --quiet firewalld && systemctl is-active --quiet systemd-resolved" \
    >/dev/null 2>&1; then
    rollback_ready=true
    break
  fi
  kill -0 "$active_qemu_pid" >/dev/null 2>&1 || break
  sleep 2
done
[[ $rollback_ready == true ]] || {
  collect_guest_diagnostics reboot-rollback
  echo "boot recovery did not restore WAN SSH, the transaction and original host services" >&2
  exit 1
}
wan_ssh 'test ! -e /root/.lkit/state/install-state.json'
wan_ssh 'test ! -e /srv/landscape/data'
wan_ssh 'test -f /var/lib/lkit-qemu/retry-checked && test -f /var/lib/lkit-qemu/ra-observed'
wan_ssh 'nmcli -t -f UUID,DEVICE connection show --active | grep -q "^1c776a18-1caf-4ac8-8f46-5573fb8db501:"'
# Variables and command substitutions are evaluated inside the guest.
# shellcheck disable=SC2016
wan_ssh 'test "$(readlink /etc/systemd/system/lkit-qemu-relative.target.wants/NetworkManager.service)" = ../NetworkManager.service'
# Variables and command substitutions are evaluated inside the guest.
# shellcheck disable=SC2016
wan_ssh 'test "$(readlink /etc/systemd/system/lkit-qemu-absolute.target.wants/NetworkManager.service)" = "$(cat /var/lib/lkit-qemu/network-manager-vendor)"'
cleanup_vm

boot_vm confirm
start_takeover confirm
lan_ssh '/usr/local/bin/lkit network confirm' \
  >"$artifact_dir/confirm-command.log" 2>&1
assert_confirmed_wan_plan confirm
lan_ssh "jq -e '.phase == \"committed\"' /root/.lkit/transactions/*.json" >/dev/null
lan_ssh "jq -e '.active_version != null' /root/.lkit/state/install-state.json" >/dev/null
lan_ssh "! find /etc/systemd/system -maxdepth 1 -name 'lkit-network-*' | grep -q ."
run_real_backup_journey
lan_ssh 'systemctl poweroff' >/dev/null 2>&1 || true

for _ in $(seq 1 60); do
  kill -0 "$active_qemu_pid" >/dev/null 2>&1 || break
  sleep 1
done
kill -0 "$active_qemu_pid" >/dev/null 2>&1 && {
  echo "confirmed VM did not power off" >&2
  exit 1
}
active_qemu_pid=
cleanup_vm

boot_vm legacy-upgrade
wan_ssh 'python3 /var/lib/lkit-qemu/upgrade-guest.py prepare' | tee -a "$artifact_dir/upgrade-assertions.log"
start_takeover legacy-upgrade
lan_ssh '/usr/local/bin/lkit network confirm' | tee -a "$artifact_dir/upgrade-assertions.log"
upgrade_guest_async move-to-disk
journey_guest identity
journey_api seed
upgrade_guest_async upgrade
journey_api assert
reboot_restored_vm present
lan_ssh 'python3 /var/lib/lkit-qemu/upgrade-guest.py assert-rebooted' | tee -a "$artifact_dir/upgrade-assertions.log"
journey_api assert
assert_confirmed_wan_plan upgraded
lan_ssh 'systemctl poweroff' >/dev/null 2>&1 || true
cleanup_vm

echo "QEMU official installation, upgrade, guarded recovery, power-loss and business acceptance passed"
