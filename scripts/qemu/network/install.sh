#!/usr/bin/env bash
# Run the real CLI inside the disposable guest with private release credentials.
set -euo pipefail
[[ -f /etc/lkit-qemu-network ]] || { echo 'Not a QEMU network acceptance guest' >&2; exit 2; }
if [[ -f /var/lib/lkit-qemu/github.env ]]; then
  set -a
  # shellcheck disable=SC1091
  source /var/lib/lkit-qemu/github.env
  set +a
fi
exec /usr/local/bin/lkit install "$@"
