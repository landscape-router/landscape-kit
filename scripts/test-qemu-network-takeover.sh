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
  sha256sum ssh ssh-keygen timeout
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
  --include=systemd,systemd-sysv,dbus,linux-image-cloud-amd64,openssh-server,network-manager,firewalld,systemd-resolved,ppp,iproute2,nftables,curl,ca-certificates,jq,procps,kmod \
  trixie "$rootfs" https://deb.debian.org/debian

install -D -m 0755 "$lkit_binary" "$rootfs/usr/local/bin/lkit"
install -d -m 0700 "$rootfs/root/.ssh"
printf '%s\n' "$public_key" >"$rootfs/root/.ssh/authorized_keys"
chmod 0600 "$rootfs/root/.ssh/authorized_keys"
install -d -m 0755 "$rootfs/etc/ssh/sshd_config.d"
printf '%s\n' 'PermitRootLogin prohibit-password' >"$rootfs/etc/ssh/sshd_config.d/90-lkit-qemu.conf"
printf '%s\n' 'lkit-qemu' >"$rootfs/etc/hostname"
printf '%s\n' '/dev/vda / ext4 defaults 0 1' >"$rootfs/etc/fstab"
install -d -m 0700 "$rootfs/var/lib/lkit-qemu"
printf '%s\n' 'Secret123' >"$rootfs/var/lib/lkit-qemu/password"
chmod 0600 "$rootfs/var/lib/lkit-qemu/password"

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
    if lan_ssh "jq -e '.phase == \"awaiting_network_confirmation\"' /var/lib/landscape/transactions/*.json" \
      >/dev/null 2>&1; then
      return 0
    fi
    if lan_ssh "jq -e '.phase == \"failed\" or .phase == \"rolled_back\"' /var/lib/landscape/transactions/*.json" \
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
  ssh "${ssh_common[@]}" root@192.168.10.1 "$@"
}

collect_guest_diagnostics() {
  local scenario=$1
  local output=$artifact_dir/$scenario-guest-diagnostics.log
  local diagnostic_command='set +e
echo "== landscape journal =="
journalctl -b --no-pager -u landscape-router.service
echo "== landscape files =="
find /var/lib/landscape -maxdepth 4 -type f -printf "%p\n" | sort
echo "== landscape logs =="
find /var/lib/landscape/data/logs -maxdepth 1 -type f -exec sh -c '\''for file do echo "--- $file"; tail -n 300 "$file"; done'\'' sh {} +
echo "== transactions =="
for file in /var/lib/landscape/transactions/*.json; do test -f "$file" && { echo "--- $file"; cat "$file"; }; done
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

boot_vm() {
  local scenario=$1
  local disk=$test_root/$scenario.ext4
  cp --reflink=auto --sparse=always "$base_disk" "$disk"
  active_tap=lkitlan0
  ip tuntap add dev "$active_tap" mode tap
  ip address add 192.168.10.2/24 dev "$active_tap"
  ip link set "$active_tap" up
  active_ssh_port=$(next_port)
  qemu-system-x86_64 \
    -name "lkit-$scenario" \
    -machine q35,accel=kvm \
    -cpu host \
    -smp 2 \
    -m 4096 \
    -kernel "$kernel" \
    -initrd "$initrd" \
    -append 'root=/dev/vda rw console=ttyS0 systemd.log_target=console' \
    -drive "file=$disk,format=raw,if=virtio,cache=unsafe" \
    -netdev "user,id=wan,hostfwd=tcp:127.0.0.1:${active_ssh_port}-:22" \
    -device virtio-net-pci,netdev=wan,mac=52:54:00:12:34:01 \
    -netdev "tap,id=lan,ifname=$active_tap,script=no,downscript=no" \
    -device virtio-net-pci,netdev=lan,mac=52:54:00:12:34:02 \
    -display none \
    -monitor none \
    -serial stdio \
    >"$artifact_dir/$scenario-serial.log" 2>&1 &
  active_qemu_pid=$!
  wait_for_ssh wan || {
    echo "$scenario VM did not expose SSH through its WAN" >&2
    return 1
  }
  wan_ssh 'systemctl set-environment RUST_BACKTRACE=1 LANDSCAPE_LOG_TERMINAL=true'
  wan_ssh 'systemctl is-active NetworkManager firewalld systemd-resolved ssh' >/dev/null
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
  local version_args=()
  if [[ -n ${LKIT_QEMU_LANDSCAPE_VERSION:-} ]]; then
    version_args=(--version "$LKIT_QEMU_LANDSCAPE_VERSION")
  fi
  set +e
  printf '%s\n1\n\n\n\n' "$wan_index" | timeout 900 ssh "${ssh_common[@]}" \
    -tt -p "$active_ssh_port" root@127.0.0.1 \
    /usr/local/bin/lkit install --takeover-network \
    --install-dir /var/lib/landscape --service-manager systemd \
    --admin-user admin --password-file /var/lib/lkit-qemu/password \
    "${version_args[@]}" \
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
  assert_lan "$scenario" "WAN has no IPv4 address" \
    "! ip -4 -o address show dev \$(jq -r '.network_takeover.plan.mode.wan' /var/lib/landscape/transactions/*.json) | grep -q ' inet '"
  for unit in NetworkManager.service firewalld.service systemd-resolved.service; do
    assert_lan "$scenario" "$unit is masked" \
      "test \"\$(systemctl is-enabled $unit || true)\" = masked"
    assert_lan "$scenario" "$unit is inactive" \
      "test \"\$(systemctl is-active $unit || true)\" = inactive"
  done
}

boot_vm reboot-rollback
start_takeover reboot-rollback
lan_ssh 'systemctl reboot' >/dev/null 2>&1 || true
rollback_ready=false
rollback_deadline=$((SECONDS + 300))
while (( SECONDS < rollback_deadline )); do
  if wan_ssh \
    "jq -e '.phase == \"rolled_back\"' /var/lib/landscape/transactions/*.json >/dev/null && systemctl is-active --quiet NetworkManager firewalld systemd-resolved" \
    >/dev/null 2>&1; then
    rollback_ready=true
    break
  fi
  kill -0 "$active_qemu_pid" >/dev/null 2>&1 || break
  sleep 2
done
[[ $rollback_ready == true ]] || {
  echo "boot recovery did not restore the original host network services" >&2
  exit 1
}
wan_ssh 'test ! -e /var/lib/landscape/state/install-state.json'
cleanup_vm

boot_vm confirm
start_takeover confirm
lan_ssh '/usr/local/bin/lkit network confirm --install-dir /var/lib/landscape' \
  >"$artifact_dir/confirm-command.log" 2>&1
lan_ssh "jq -e '.phase == \"committed\"' /var/lib/landscape/transactions/*.json" >/dev/null
lan_ssh "jq -e '.active_version != null' /var/lib/landscape/state/install-state.json" >/dev/null
lan_ssh "! find /etc/systemd/system -maxdepth 1 -name 'lkit-network-*' | grep -q ."
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

echo "QEMU network takeover confirmation and reboot rollback passed"
