#!/usr/bin/env bash
# CI QEMU/KVM lifecycle checks: real Debian SysVinit, production lkit.
# The only network is restricted user-mode SSH; no host networking is changed.
set -euo pipefail
[[ ${LKIT_E2E:-0} == 1 ]] || { echo 'Requires explicit LKIT_E2E=1' >&2; exit 2; }
[[ $(uname -s):$(uname -m) == Linux:x86_64 && $EUID == 0 ]] || {
  echo 'Requires Linux x86_64 root' >&2; exit 2;
}
[[ -c /dev/kvm && -r /dev/kvm && -w /dev/kvm ]] || {
  echo 'KVM is required; no TCG fallback' >&2; exit 2;
}
for tool in curl dpkg-deb mke2fs mmdebstrap python3 qemu-system-x86_64 sha256sum ssh ssh-keygen timeout; do
  command -v "$tool" >/dev/null || { echo "Missing $tool" >&2; exit 2; }
done
repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
work=$(mktemp -d /var/tmp/lkit-qemu-sysvinit.XXXXXX)
artifacts=${LKIT_QEMU_ARTIFACT_DIR:-$repo_root/target/qemu-sysvinit-artifacts}
mkdir -p "$artifacts"
artifacts=$(realpath "$artifacts")
qemu_pid=
ssh_port=
ssh_options=()
guest() { timeout 180 ssh "${ssh_options[@]}" -p "$ssh_port" root@127.0.0.1 "$@"; }

diagnostics() {
  [[ -n $qemu_pid ]] || return 0
  # shellcheck disable=SC2016
  timeout 20 ssh "${ssh_options[@]}" -p "$ssh_port" root@127.0.0.1 '
    set +e
    uname -a
    readlink /proc/1/exe
    runlevel
    ps -ef
    cat /proc/self/mountinfo
    find /etc/rc?.d -name "*lkit.service" -ls
    for file in /var/log/lkit-qemu/* /root/.lkit/run/*.pid; do
      test -f "$file" && { echo "--- $file"; cat "$file"; }
    done
  ' >"$artifacts/guest-diagnostics.log" 2>&1 || true
  timeout 20 ssh "${ssh_options[@]}" -p "$ssh_port" root@127.0.0.1 \
    'tar -C /var/log -czf - lkit-qemu' >"$artifacts/guest-logs.tar.gz" 2>"$artifacts/guest-log-copy.log" || true
  timeout 10 ssh "${ssh_options[@]}" -p "$ssh_port" root@127.0.0.1 \
    'cat /var/log/lkit-qemu/results.tsv' >"$artifacts/results.tsv" 2>/dev/null || true
}

cleanup() {
  local status=$?
  trap - EXIT
  diagnostics
  if [[ -n $qemu_pid ]]; then
    kill "$qemu_pid" >/dev/null 2>&1 || true
    wait "$qemu_pid" >/dev/null 2>&1 || true
  fi
  rm -rf "$work"
  exit "$status"
}
trap cleanup EXIT

if [[ -n ${LKIT_QEMU_PREBUILT:-} ]]; then
  lkit_binary=$(realpath "$LKIT_QEMU_PREBUILT")
else
  (cd "$repo_root" && CARGO_TARGET_DIR="$work/target" cargo build --locked --release -p lkit-cli --bin lkit)
  lkit_binary=$work/target/release/lkit
fi
[[ -x $lkit_binary ]] || { echo "Missing executable: $lkit_binary" >&2; exit 2; }
sha256sum "$lkit_binary" >"$artifacts/lkit.sha256"
ssh-keygen -q -t ed25519 -N '' -f "$work/id_ed25519"
rootfs=$work/rootfs

curl --fail --location --silent --show-error --retry 3 \
  https://deb.debian.org/debian/pool/main/d/debian-archive-keyring/debian-archive-keyring_2025.1_all.deb \
  --output "$work/keyring.deb"
printf '%s  %s\n' 9ea7778e443144ca490668737a8ab22dd3e748bb99e805e22ec055abeb3c7fac \
  "$work/keyring.deb" | sha256sum --check --status
dpkg-deb --extract "$work/keyring.deb" "$work/keyring"
mmdebstrap --variant=minbase --architectures=amd64 \
  --keyring="$work/keyring/usr/share/keyrings/debian-archive-keyring.gpg" \
  --include=sysvinit-core,sysv-rc,initscripts,orphan-sysvinit-scripts,udev,linux-image-cloud-amd64,initramfs-tools,openssh-server,ifupdown,iproute2,procps,kmod,e2fsprogs,ca-certificates \
  trixie "$rootfs" https://deb.debian.org/debian >"$artifacts/image-build.log" 2>&1

install -D -m 0755 "$lkit_binary" "$rootfs/usr/local/bin/lkit"
install -D -m 0755 "$repo_root/scripts/qemu/sysvinit/guest-tests.sh" "$rootfs/root/guest-tests.sh"
install -D -m 0600 "$work/id_ed25519.pub" "$rootfs/root/.ssh/authorized_keys"
chmod 0700 "$rootfs/root/.ssh"
install -d -m 0755 "$rootfs/etc/ssh/sshd_config.d"
printf '%s\n' 'PermitRootLogin prohibit-password' 'PasswordAuthentication no' \
  >"$rootfs/etc/ssh/sshd_config.d/90-lkit-qemu.conf"
printf '%s\n' lkit-sysvinit-ci >"$rootfs/etc/hostname"
printf '%s\n' 'Disposable QEMU guest' >"$rootfs/etc/lkit-ci-sysvinit"
printf '%s\n' '/dev/vda / ext4 defaults 0 1' >"$rootfs/etc/fstab"
printf '%s\n' 'auto lo' 'iface lo inet loopback' 'auto eth0' \
  'iface eth0 inet static' '  address 10.0.2.15/24' '  gateway 10.0.2.2' \
  >"$rootfs/etc/network/interfaces"
printf '%s\n' sysvinit-qemu-recovery-secret >"$rootfs/root/ci-flare.psk"
chmod 0600 "$rootfs/root/ci-flare.psk"
chroot "$rootfs" update-rc.d ssh defaults
chroot "$rootfs" update-rc.d networking defaults
chroot "$rootfs" dpkg-query -W >"$artifacts/guest-packages.tsv"
kernel=$(find "$rootfs/boot" -maxdepth 1 -type f -name 'vmlinuz-*' | sort | tail -1)
initrd=$(find "$rootfs/boot" -maxdepth 1 -type f -name 'initrd.img-*' | sort | tail -1)
[[ -n $kernel && -n $initrd ]] || { echo 'Missing guest kernel/initrd' >&2; exit 1; }
truncate -s 4G "$work/root.ext4"
mke2fs -q -t ext4 -F -d "$rootfs" "$work/root.ext4"

ssh_port=$(python3 - <<'PY'
import socket
with socket.socket() as sock:
    sock.bind(('127.0.0.1', 0))
    print(sock.getsockname()[1])
PY
)
ssh_options=(-i "$work/id_ed25519" -o BatchMode=yes -o ConnectTimeout=3
  -o ServerAliveInterval=2 -o ServerAliveCountMax=3
  -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null)
qemu-system-x86_64 -name lkit-sysvinit-ci -machine q35,accel=kvm -cpu host -smp 2 -m 2048 \
  -kernel "$kernel" -initrd "$initrd" \
  -append 'root=/dev/vda rw console=ttyS0 net.ifnames=0 biosdevname=0' \
  -drive "file=$work/root.ext4,format=raw,if=virtio" \
  -netdev "user,id=mgmt,restrict=on,ipv6=off,hostfwd=tcp:127.0.0.1:$ssh_port-:22" \
  -device virtio-net-pci,netdev=mgmt,mac=52:54:00:12:34:01 \
  -display none -monitor none -serial stdio >"$artifacts/serial.log" 2>&1 &
qemu_pid=$!

wait_for_boot() {
  local previous=${1:-} boot attempt
  for ((attempt = 0; attempt < 120; attempt++)); do
    kill -0 "$qemu_pid" 2>/dev/null || { echo 'QEMU exited' >&2; return 1; }
    boot=$(timeout 6 ssh "${ssh_options[@]}" -p "$ssh_port" root@127.0.0.1 \
      'cat /proc/sys/kernel/random/boot_id' 2>/dev/null) || boot=
    if [[ -n $boot && $boot != "$previous" ]]; then return 0; fi
    sleep 1
  done
  echo 'Timed out waiting for a new guest boot' >&2
  return 1
}

reboot_guest() {
  local previous
  previous=$(guest 'cat /proc/sys/kernel/random/boot_id')
  guest 'nohup sh -c "sleep 1; /sbin/reboot" </dev/null >/var/log/lkit-qemu/reboot.log 2>&1 &' || true
  wait_for_boot "$previous"
}

wait_for_boot
guest '/root/guest-tests.sh initial' 2>&1 | tee "$artifacts/initial.log"
reboot_guest
guest '/root/guest-tests.sh enabled-reboot' 2>&1 | tee "$artifacts/enabled-reboot.log"
reboot_guest
guest '/root/guest-tests.sh disabled-reboot' 2>&1 | tee "$artifacts/disabled-reboot.log"
guest 'cat /var/log/lkit-qemu/results.tsv' >"$artifacts/results.tsv"
echo "SysVinit QEMU lifecycle passed; artifacts: $artifacts"
