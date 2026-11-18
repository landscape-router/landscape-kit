#!/usr/bin/env bash
# CI, called after the two containerlab HTTP markers and fingerprints have been verified.
set -euo pipefail
umask 077
work=$(mktemp -d)
tap=fr-win
vm_pid=
tap_created=false
# ShellCheck 0.10 does not follow EXIT traps.
# shellcheck disable=SC2317
cleanup() {
  if [[ -n $vm_pid ]]; then
    kill "$vm_pid" 2>/dev/null || true
    wait "$vm_pid" 2>/dev/null || true
  fi
  if [[ $tap_created == true ]]; then sudo ip link del "$tap"; fi
  rm -rf "$work"
}
trap cleanup EXIT
[[ $1 =~ ^[a-f0-9]{64}$ && $2 =~ ^[a-f0-9]{64}$ && $1 != "$2" ]]
mkdir "$work/cd"
cp windows-client/lflare.exe scripts/flare/windows-npcap-smoke.ps1 "$work/cd/"
nonce=$(cat /proc/sys/kernel/random/uuid)
jq -n --arg a "$1" --arg b "$2" --arg nonce "$nonce" '{a:$a,b:$b,nonce:$nonce}' >"$work/cd/targets.json"
printf '%s\n' fixture-root-password >"$work/cd/password.txt"
genisoimage -quiet -J -R -V FLARETEST -o "$work/test.iso" "$work/cd"
sudo ip tuntap add dev "$tap" mode tap user "$(id -un)"
tap_created=true
sudo ip link set "$tap" master lan
sudo ip link set "$tap" up
# The guest has exactly one NIC, on the same isolated LAN as A and B.
# The runner user owns the TAP; kvm access is granted only for this disposable job.
sudo setfacl -m "u:$(id -un):rw" /dev/kvm
qemu-system-x86_64 -machine pc,accel=kvm -cpu host -smp 2 -m 4096 \
  -drive "file=$FLARE_WINDOWS_IMAGE,if=ide,format=qcow2,snapshot=on" -boot order=c \
  -cdrom "$work/test.iso" -netdev "tap,id=lab,ifname=$tap,script=no,downscript=no" \
  -device e1000,netdev=lab,mac=52:54:00:fa:00:03 -serial "file:$work/result" \
  -display none -monitor none -no-reboot &
vm_pid=$!
for _ in $(seq 1 180); do
  if [[ -e $work/result ]]; then
    if grep -Fq "FAIL $nonce" "$work/result"; then cat "$work/result" >&2; exit 1; fi
    if grep -Fq "DONE $nonce" "$work/result"; then
      grep -F "PASS $1 returned HTTP marker A" "$work/result"
      grep -F "PASS $2 returned HTTP marker B" "$work/result"
      exit 0
    fi
  fi
  kill -0 "$vm_pid" || { echo 'Windows stopped before returning both markers' >&2; exit 1; }
  sleep 3
done
echo 'Windows did not report a result over COM1' >&2
exit 1
