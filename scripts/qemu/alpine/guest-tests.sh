#!/bin/sh
# Runs only inside the disposable Alpine VM, never on the CI host.
set -eu
[ -f /etc/lkit-ci-alpine ] || { echo 'Not an Alpine CI guest' >&2; exit 2; }

wait_for() {
    attempts=0
    until "$@"; do
        attempts=$((attempts + 1))
        [ "$attempts" -lt 60 ] || { echo "Timed out: $*" >&2; return 1; }
        sleep 1
    done
}

has_log() { grep -E "$1" /var/log/lkit.log >/dev/null; }
bpf_rw() { awk '$5 == "/sys/fs/bpf" && $6 ~ /(^|,)rw(,|$)/ && / - bpf / { found=1 } END { exit !found }' /proc/self/mountinfo; }
bpf_ro() { awk '$5 == "/sys/fs/bpf" && $6 ~ /(^|,)ro(,|$)/ && / - bpf / { found=1 } END { exit !found }' /proc/self/mountinfo; }
mount_count() { awk '$5 == "/sys/fs/bpf" { n++ } END { print n+0 }' /proc/self/mountinfo; }

assert_cgroup_ready() {
    rc-service cgroups status
    test -L /etc/runlevels/boot/cgroups
    test "$(readlink -f /etc/runlevels/boot/cgroups)" = /etc/init.d/cgroups
    awk '$5 == "/sys/fs/cgroup" && $6 ~ /(^|,)rw(,|$)/ && / - cgroup2 / { found=1 } END { exit !found }' /proc/self/mountinfo
    grep -w cpu /sys/fs/cgroup/cgroup.controllers
}

assert_init_report() {
    report=$1
    /usr/local/bin/lkit --lang en check > "$report" 2>&1 || true
    grep -F 'Init system and startup' "$report"
    grep -E '^PASS.*init[.]backend.*OpenRC' "$report"
    grep -E '^PASS.*openrc[.]cgroup_boot.*boot' "$report"
    for service in network_manager systemd_resolved firewalld; do
        grep -E "^PASS.*service[.]${service}[[:space:]]" "$report"
    done
    grep -F 'OpenRC requires cgroups in the boot runlevel' "$report"
    grep -F 'The cgroups service is registered in the boot runlevel' "$report"
    grep -F 'NetworkManager is not installed' "$report"
    grep -F 'systemd-resolved is not running or enabled' "$report"
    grep -F 'firewalld is not running or enabled' "$report"
}

assert_landscape_starts() {
    rm -f /run/landscape-order-ok
    rc-service landscape-router.service start
    wait_for test -f /run/landscape-order-ok
    rc-service landscape-router.service status
    rc-service landscape-router.service stop
    rm -f /run/landscape-order-ok
}

assert_flare() {
    scenario=$1
    timeout 40 ip netns exec ci-flare \
        /usr/local/bin/lflare cli --dev ci-client --psk alpine-qemu-recovery-secret \
        >"/var/log/client-$scenario.log" 2>&1 &
    client_pid=$!
    attempts=0
    until grep 'session .* established' "/var/log/client-$scenario.log" >/dev/null; do
        attempts=$((attempts + 1))
        if [ "$attempts" -ge 30 ] || ! kill -0 "$client_pid" 2>/dev/null; then
            cat "/var/log/client-$scenario.log" >&2
            kill "$client_pid" 2>/dev/null || true
            wait "$client_pid" || true
            return 1
        fi
        sleep 1
    done
    kill "$client_pid" 2>/dev/null || true
    wait "$client_pid" || true
    has_log 'authenticated, session'
}

assert_docker_ready() {
    wait_for docker info
    rc-service docker status
    docker buildx version
    docker compose version
}

case "${1:-}" in
    initial)
        echo '== OpenRC identity and mandatory Cgroup boot registration =='
        assert_cgroup_ready
        # Current mounts remain intact: boot registration is a separate requirement.
        rc-update del cgroups boot
        /usr/local/bin/lkit --lang en check > /var/log/check-no-cgroup-boot.log 2>&1 || true
        rc-update add cgroups boot
        grep -E '^ERROR.*openrc[.]cgroup_boot' /var/log/check-no-cgroup-boot.log
        grep -F 'Cgroups must start at boot before Landscape' /var/log/check-no-cgroup-boot.log
        assert_init_report /var/log/check-init-ready.log

        echo '== actual bpffs mount through production self install =='
        test "$(mount_count)" = 0
        /usr/local/bin/lkit --lang en --non-interactive self install --flare-psk-file /root/ci-flare.psk
        test -x /etc/init.d/lkit.service
        rc-service lkit.service status
        wait_for bpf_rw
        wait_for has_log 'mounted bpffs at /sys/fs/bpf'
        test "$(mount_count)" = 1
        test "$(stat -c %a /var/log/lkit.log)" = 600
        /usr/local/bin/lkit --lang en check > /var/log/check-ready.log 2>&1 || true
        grep -F 'bpffs is mounted read-write at /sys/fs/bpf' /var/log/check-ready.log
        assert_flare mounted

        echo '== repeated self install must restart without stacking mounts =='
        before=$(cat /run/lkit.pid)
        /usr/local/bin/lkit --lang en --non-interactive self install
        after=$(cat /run/lkit.pid)
        test "$before" != "$after"
        wait_for bpf_rw
        test "$(mount_count)" = 1
        test "$(grep -c 'mounted bpffs at /sys/fs/bpf' /var/log/lkit.log)" = 1

        echo '== conflicting filesystem must preserve daemon and flare =='
        rc-service lkit.service stop
        umount /sys/fs/bpf
        mount -t tmpfs -o ro tmpfs /sys/fs/bpf
        : > /var/log/lkit.log
        rc-service lkit.service start
        wait_for has_log 'another filesystem is mounted; not replacing it.*continuing daemon startup'
        rc-service lkit.service status
        awk '$5 == "/sys/fs/bpf" && / - tmpfs / { found=1 } END { exit !found }' /proc/self/mountinfo
        /usr/local/bin/lkit --lang en check > /var/log/check-conflict.log 2>&1 || true
        grep -F 'Another filesystem is mounted at /sys/fs/bpf' /var/log/check-conflict.log
        assert_flare conflict
        assert_landscape_starts

        echo '== read-only bpffs must not be remounted or block flare =='
        rc-service lkit.service stop
        umount /sys/fs/bpf
        mount -t bpf -o ro bpf /sys/fs/bpf
        : > /var/log/lkit.log
        rc-service lkit.service start
        wait_for has_log 'bpffs is read-only; not remounting.*continuing daemon startup'
        bpf_ro
        assert_flare readonly
        assert_landscape_starts
        bpf_ro
        rc-service lkit.service stop
        umount /sys/fs/bpf

        # Both services start in the default runlevel after the test-only boot
        # service removes any bpffs mounted by Alpine's sysfs service.
        rc-update add landscape-router.service default

        echo '== real Alpine Docker service and CLI plugins =='
        rc-update add docker default
        rc-service docker start
        assert_docker_ready
        test -L /etc/runlevels/default/docker
        rc-service docker stop
        ;;
    rebooted)
        echo '== persistent Cgroup and current init report after reboot =='
        assert_cgroup_ready
        assert_init_report /var/log/check-init-rebooted.log
        echo '== real OpenRC boot ordering and bpffs fallback after reboot =='
        wait_for bpf_rw
        rc-service lkit.service status
        wait_for test -f /run/landscape-order-ok
        rc-service landscape-router.service status
        has_log 'mounted bpffs at /sys/fs/bpf'
        test "$(mount_count)" = 1
        assert_flare rebooted

        echo '== real Docker daemon starts through OpenRC after reboot =='
        assert_docker_ready
        rc-service docker stop
        rc-update del docker default

        echo '== self remove is idempotent and leaves shared bpffs mounted =='
        rc-service landscape-router.service stop
        /usr/local/bin/lkit --lang en self remove
        /usr/local/bin/lkit --lang en self remove
        test ! -e /etc/init.d/lkit.service
        test -x /usr/local/bin/lkit
        bpf_rw
        echo 'ALPINE BPFFS AND OPENRC E2E PASSED'
        ;;
    *) echo 'expected initial or rebooted' >&2; exit 2 ;;
esac
