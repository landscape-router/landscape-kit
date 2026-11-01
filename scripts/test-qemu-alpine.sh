#!/usr/bin/env bash
# CI-only real Alpine/OpenRC test. All mounts and raw networking are in the VM.
set -euo pipefail
[[ ${LKIT_E2E:-0} == 1 ]] || { echo 'Requires explicit LKIT_E2E=1 (CI only)' >&2; exit 2; }
[[ $(uname -s):$(uname -m) == Linux:x86_64 && $EUID == 0 ]] || { echo 'Requires Linux x86_64 root' >&2; exit 2; }
[[ -r /dev/kvm && -w /dev/kvm ]] || { echo 'KVM is required; no TCG fallback' >&2; exit 2; }
for tool in curl docker install mke2fs musl-gcc python3 qemu-system-x86_64 sha256sum ssh ssh-keygen tar timeout truncate; do
  command -v "$tool" >/dev/null || { echo "Missing $tool" >&2; exit 2; }
done
: "${LKIT_QEMU_ALPINE_BIN_DIR:?directory containing production lkit-x86_64 and lflare-linux-x86_64}"
: "${LKIT_QEMU_RENDERER:?native renderer built from scripts/qemu/alpine/render-services.rs}"
repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
work=$(mktemp -d /var/tmp/lkit-qemu-alpine.XXXXXX)
artifacts=${LKIT_QEMU_ARTIFACT_DIR:-$repo_root/target/qemu-alpine-artifacts}
mkdir -p "$artifacts"
artifacts=$(realpath "$artifacts")
image="lkit-alpine-ci:${GITHUB_RUN_ID:-$$}-${GITHUB_RUN_ATTEMPT:-0}"
container=
qemu_pid=
ssh_port=
ssh_options=()

guest() { timeout 180 ssh "${ssh_options[@]}" -p "$ssh_port" root@127.0.0.1 "$@"; }

diagnostics() {
  if [[ -n $qemu_pid ]]; then
    # Variables in this command are expanded by the guest shell.
    # shellcheck disable=SC2016
    timeout 25 ssh "${ssh_options[@]}" -p "$ssh_port" root@127.0.0.1 '
      set +e
      uname -a
      cat /proc/self/mountinfo
      rc-status -a
      ps -ef
      for file in /var/log/lkit.log /var/log/rc.log /var/log/ci-prepare.log /var/log/client-*.log /var/log/check-*.log; do
        test -f "$file" && { echo "--- $file"; cat "$file"; }
      done
    ' >"$artifacts/guest-diagnostics.log" 2>&1 || true
  fi
}

cleanup() {
  local status=$?
  trap - EXIT
  diagnostics
  if [[ -n $qemu_pid ]]; then
    kill "$qemu_pid" >/dev/null 2>&1 || true
    wait "$qemu_pid" >/dev/null 2>&1 || true
  fi
  [[ -z $container ]] || docker rm -f "$container" >/dev/null 2>&1 || true
  docker image rm "$image" >/dev/null 2>&1 || true
  rm -rf "$work"
  exit "$status"
}
trap cleanup EXIT

mkdir -p "$work/context" "$work/rootfs"
curl --fail --location --silent --show-error --retry 3 \
  https://dl-cdn.alpinelinux.org/alpine/v3.22/releases/x86_64/alpine-minirootfs-3.22.2-x86_64.tar.gz \
  -o "$work/context/alpine-minirootfs.tar.gz"
printf '%s  %s\n' 412454ed98025ad9cd910d13f3d448b184e81502baa83180e89f98d0f13674be \
  "$work/context/alpine-minirootfs.tar.gz" | sha256sum --check --status
install -m 0644 "$repo_root/scripts/qemu/alpine/Dockerfile" "$work/context/Dockerfile"
docker build -t "$image" "$work/context" >"$artifacts/image-build.log" 2>&1
container=$(docker create "$image")
docker export "$container" | tar -xpf - -C "$work/rootfs"
docker rm "$container" >/dev/null
container=
rootfs=$work/rootfs
rm -f "$rootfs/.dockerenv"

install -D -m 0755 "$LKIT_QEMU_ALPINE_BIN_DIR/lkit-x86_64" "$rootfs/usr/local/bin/lkit"
install -D -m 0755 "$LKIT_QEMU_ALPINE_BIN_DIR/lflare-linux-x86_64" "$rootfs/usr/local/bin/lflare"
install -D -m 0755 "$repo_root/scripts/qemu/alpine/guest-tests.sh" "$rootfs/root/guest-tests.sh"
install -D -m 0755 "$repo_root/scripts/qemu/alpine/prepare.openrc" "$rootfs/etc/init.d/ci-prepare"
ln -s /etc/init.d/ci-prepare "$rootfs/etc/runlevels/boot/ci-prepare"
mkdir -p "$rootfs/srv/landscape/current/static" "$rootfs/srv/landscape/data"
musl-gcc -static -Os -Wall -Wextra -Werror "$repo_root/scripts/qemu/alpine/router-fixture.c" \
  -o "$rootfs/srv/landscape/current/landscape-webserver"
"$LKIT_QEMU_RENDERER" "$rootfs/etc/init.d/landscape-router.service"
chmod 0755 "$rootfs/etc/init.d/landscape-router.service"

ssh-keygen -q -t ed25519 -N '' -f "$work/id_ed25519"
install -D -m 0600 "$work/id_ed25519.pub" "$rootfs/root/.ssh/authorized_keys"
chmod 0700 "$rootfs/root/.ssh"
printf '%s\n' 'PermitRootLogin prohibit-password' 'PasswordAuthentication no' \
  'PubkeyAuthentication yes' > "$rootfs/etc/ssh/sshd_config"
printf '%s\n' lkit-alpine-ci > "$rootfs/etc/hostname"
printf '%s\n' 'Disposable QEMU guest; not a production system' > "$rootfs/etc/lkit-ci-alpine"
printf '%s\n' '/dev/vda / ext4 defaults 0 0' > "$rootfs/etc/fstab"
printf '%s\n' 'virtio_net' 'veth' >> "$rootfs/etc/modules"
printf '%s\n' 'auto lo' 'iface lo inet loopback' 'auto eth0' 'iface eth0 inet dhcp' > "$rootfs/etc/network/interfaces"
printf '%s\n' 'rc_logger="YES"' 'rc_log_path="/var/log/rc.log"' 'rc_parallel="YES"' >> "$rootfs/etc/rc.conf"
printf '%s\n' alpine-qemu-recovery-secret > "$rootfs/root/ci-flare.psk"
chmod 0600 "$rootfs/root/ci-flare.psk"

truncate -s 2G "$work/root.ext4"
mke2fs -q -t ext4 -F -d "$rootfs" "$work/root.ext4"
ssh_port=$(python3 - <<'PY'
import socket
with socket.socket() as sock:
    sock.bind(("127.0.0.1", 0))
    print(sock.getsockname()[1])
PY
)
ssh_options=(-i "$work/id_ed25519" -o BatchMode=yes -o ConnectTimeout=3 \
  -o ServerAliveInterval=2 -o ServerAliveCountMax=3 -o StrictHostKeyChecking=no \
  -o UserKnownHostsFile=/dev/null -o LogLevel=ERROR)
qemu-system-x86_64 -enable-kvm -cpu host -smp 2 -m 1536 \
  -kernel "$rootfs/boot/vmlinuz-virt" -initrd "$rootfs/boot/initramfs-virt" \
  -append 'root=/dev/vda rootfstype=ext4 rw console=ttyS0,115200' \
  -drive "file=$work/root.ext4,format=raw,if=virtio" \
  -netdev "user,id=mgmt,restrict=on,hostfwd=tcp:127.0.0.1:$ssh_port-:22" \
  -device virtio-net-pci,netdev=mgmt -display none -monitor none \
  -serial "file:$artifacts/serial.log" >"$artifacts/qemu.log" 2>&1 &
qemu_pid=$!

wait_for_boot() {
  local previous=${1:-} boot_id deadline=$((SECONDS + 240))
  while (( SECONDS < deadline )); do
    kill -0 "$qemu_pid" 2>/dev/null || return 1
    boot_id=$(guest 'cat /proc/sys/kernel/random/boot_id' 2>/dev/null) || { sleep 2; continue; }
    if [[ -n $boot_id && $boot_id != "$previous" ]]; then
      printf '%s\n' "$boot_id"
      return 0
    fi
    sleep 2
  done
  echo 'Guest did not boot within 240 seconds' >&2
  return 1
}

boot_id=$(wait_for_boot)
guest 'sh /root/guest-tests.sh initial' >"$artifacts/initial.log" 2>&1
diagnostics
mv "$artifacts/guest-diagnostics.log" "$artifacts/initial-diagnostics.log"
guest 'reboot' || true
wait_for_boot "$boot_id" > "$artifacts/reboot-id.log"
guest 'sh /root/guest-tests.sh rebooted' >"$artifacts/rebooted.log" 2>&1
echo 'ALPINE BPFFS AND OPENRC E2E PASSED'
