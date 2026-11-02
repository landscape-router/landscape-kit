#!/bin/sh
# CI-only, disposable Alpine container. Downloads are real; OpenRC startup and
# docker info use recording shims. Real daemon startup is covered in the QEMU VM.
set -eu
[ "${LKIT_E2E:-0}" = 1 ] || { echo 'Requires LKIT_E2E=1' >&2; exit 2; }
[ -f /.dockerenv ] && [ -f /etc/alpine-release ] && [ "$(id -u)" = 0 ] || {
    echo 'Requires a disposable Alpine Docker container as root' >&2
    exit 2
}

lkit=/usr/local/bin/lkit
work=$(mktemp -d)
export LKIT_ALPINE_CALL_LOG=$work/service-calls.log
export PATH="$work/shims:$PATH"
mkdir -p "$work/shims"

on_exit() {
    status=$?
    trap - 0
    if [ "$status" -ne 0 ]; then
        for log in "$work"/*.log; do
            [ -f "$log" ] || continue
            printf '\n--- %s ---\n' "$log"
            cat "$log"
        done
        cat /etc/apk/repositories
        apk info || true
    fi
    exit "$status"
}
trap on_exit 0

fail() { echo "FAIL(alpine): $*" >&2; exit 1; }
ok() { echo "PASS(alpine): $*"; }
contains() { grep -qF "$2" "$1" || fail "expected $2 in $1"; }

# Shims do not occupy the absolute paths used by Docker installation detection.
for program in rc-update rc-service; do
    cat >"$work/shims/$program" <<'SH'
#!/bin/sh
printf '%s %s\n' "${0##*/}" "$*" >>"$LKIT_ALPINE_CALL_LOG"
SH
    chmod +x "$work/shims/$program"
done
cat >"$work/shims/docker" <<'SH'
#!/bin/sh
if [ "$*" = info ]; then
    printf 'docker info\n' >>"$LKIT_ALPINE_CALL_LOG"
else
    exec /usr/bin/docker "$@"
fi
SH
chmod +x "$work/shims/docker"

"$lkit" --lang en software list >"$work/list-before.log"
contains "$work/list-before.log" 'Alpine'
contains "$work/list-before.log" '(docker) [not installed]'
# BusyBox ip/sysctl must not be mistaken for the requested full packages.
contains "$work/list-before.log" '(iproute2) [not installed]'
contains "$work/list-before.log" '(procps) [not installed]'

status=0
"$lkit" --lang en software install docker --source aliyun --yes >"$work/unsupported-source.log" 2>&1 || status=$?
[ "$status" = 2 ] || fail "Docker CE source must return usage error, got $status"
contains "$work/unsupported-source.log" 'configured apk repositories'
[ ! -e "$LKIT_ALPINE_CALL_LOG" ] || fail 'invalid source started a service'
! apk info -e docker >/dev/null 2>&1 || fail 'invalid source installed Docker'
ok 'Alpine detection and Docker CE source rejection'

# This job pins alpine:3.22; derive its matching stable branch.
release=$(cut -d. -f1,2 /etc/alpine-release)
branch=v$release
repositories=/etc/apk/repositories
printf 'https://dl-cdn.alpinelinux.org/alpine/%s/main\n' "$branch" >"$repositories"
if "$lkit" --lang en software install docker --source official --yes >"$work/no-community.log" 2>&1; then
    fail 'Docker installation without community unexpectedly succeeded'
fi
[ ! -e "$LKIT_ALPINE_CALL_LOG" ] || fail 'failed apk install started a service'
! apk info -e docker >/dev/null 2>&1 || fail 'failed apk resolution installed Docker'
ok 'missing community fails before service startup'

printf 'https://dl-cdn.alpinelinux.org/alpine/%s/community\n' "$branch" >>"$repositories"
printf '# @testing https://dl-cdn.alpinelinux.org/alpine/edge/testing\n' >>"$repositories"
cp "$repositories" "$work/original-repositories"
chmod 0640 "$repositories"
mirror=aliyun
mirror_host=mirrors.aliyun.com
"$lkit" --lang en set-mirror "$mirror" --yes >"$work/mirror.log" 2>&1 || { cat "$work/mirror.log"; fail 'switch mirror'; }
contains "$repositories" "https://$mirror_host/alpine/$branch/main"
contains "$repositories" "https://$mirror_host/alpine/$branch/community"
contains "$repositories" '# @testing https://dl-cdn.alpinelinux.org/alpine/edge/testing'
[ "$(stat -c %a "$repositories")" = 640 ] || fail 'mirror rewrite changed permissions'
backup=/var/lib/lkit/mirror-backup/alpine/etc/apk/repositories
cmp "$work/original-repositories" "$backup" || fail 'backup differs from original'
"$lkit" --lang en set-mirror --show >"$work/show.log"
contains "$work/show.log" "https://$mirror_host/alpine/$branch/main"
"$lkit" --lang en set-mirror "$mirror" --yes
cmp "$work/original-repositories" "$backup" || fail 'no-op overwrote original backup'
# lkit warns rather than failing on refresh errors, so require real apk success.
apk update
"$lkit" --lang en set-mirror --restore --yes
cmp "$work/original-repositories" "$repositories" || fail 'restore differs from original'
[ ! -d /var/lib/lkit/mirror-backup/alpine ] || fail 'restore left its backup'
"$lkit" --lang en set-mirror "$mirror" --yes
apk update
ok 'domestic mirror, real APK indexes, no-op backup and restore'

# Reproduce a minimal host with pppd but no PPPoE plugin.
apk add --no-cache ppp-daemon
! apk info -e ppp-pppoe >/dev/null 2>&1 || fail 'fixture already has the PPPoE plugin'
"$lkit" --lang en software list >"$work/pppd-only.log"
contains "$work/pppd-only.log" '(ppp) [not installed]'
"$lkit" --lang en check >"$work/check-pppoe.log" 2>&1 || true
contains "$work/check-pppoe.log" 'PPPoE plugin is missing'
"$lkit" --lang en software install base-packages --packages ppp,iproute2,iw,hostapd,procps --yes
for package in ppp ppp-pppoe iproute2 iproute2-tc iw hostapd procps-ng; do
    apk info -e "$package" >/dev/null || fail "missing base package $package"
done
for binary in pppd ip tc iw hostapd sysctl; do
    command -v "$binary" >/dev/null || fail "missing executable $binary"
done
find /usr/lib/pppd -name pppoe.so | grep -q . || fail 'missing PPPoE plugin'
"$lkit" --lang en software list >"$work/base-after.log"
for package in ppp iproute2 iw hostapd procps; do
    contains "$work/base-after.log" "($package) [installed]"
done
[ ! -e "$LKIT_ALPINE_CALL_LOG" ] || fail 'base package installation started Docker'
"$lkit" --lang en check >"$work/check-base-after.log" 2>&1 || true
contains "$work/check-base-after.log" 'The pppd command exists'
ok 'full base packages, BusyBox replacement, and pppd-only repair'

cp "$repositories" "$work/before-docker-repositories"
"$lkit" --lang en software install docker --source official --yes
for package in docker docker-openrc docker-cli-buildx docker-cli-compose; do
    apk info -e "$package" >/dev/null || fail "missing Docker package $package"
done
[ -x /usr/bin/docker ] || fail 'real Docker binary is missing'
[ -x /etc/init.d/docker ] || fail 'Docker OpenRC service definition is missing'
cmp "$work/before-docker-repositories" "$repositories" || fail 'Docker install changed repositories'
[ ! -e /etc/apt/sources.list.d/docker.list ] || fail 'Alpine wrote an apt Docker CE source'
[ ! -e /etc/yum.repos.d/docker-ce.repo ] || fail 'Alpine wrote a dnf Docker CE source'
printf 'rc-update add docker default\nrc-service docker start\ndocker info\n' >"$work/expected-calls"
cmp "$work/expected-calls" "$LKIT_ALPINE_CALL_LOG" || fail 'incorrect OpenRC/verification call order'
"$lkit" --lang en software list >"$work/list-after.log"
contains "$work/list-after.log" '(docker) [installed]'
"$lkit" --lang en set-mirror official --yes
contains "$repositories" "https://dl-cdn.alpinelinux.org/alpine/$branch/main"
apk update
ok 'Docker packages, configured mirror, OpenRC calls and installed status'
echo 'ALPINE APK MIRROR AND SOFTWARE E2E PASSED'
