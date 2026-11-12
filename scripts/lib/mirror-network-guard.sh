#!/bin/sh
# An accidental invocation must fail before Docker, probes, or package downloads.
# This is an operational opt-in, not an authorization boundary against hostile code.
lkit_require_mirror_network() {
    if [ "${LKIT_ALLOW_MIRROR_NETWORK:-0}" = 1 ] &&
       [ "${GITHUB_ACTIONS:-false}" = true ] &&
       [ "${GITHUB_EVENT_NAME:-}" = push ] &&
       [ "${GITHUB_REF:-}" = refs/heads/ci-mirror ]; then
        return 0
    fi
    echo 'Public mirror tests require a ci-mirror branch push and LKIT_ALLOW_MIRROR_NETWORK=1' >&2
    return 2
}
