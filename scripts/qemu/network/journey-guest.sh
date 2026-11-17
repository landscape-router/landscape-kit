#!/usr/bin/env bash
# Only inside the disposable Debian QEMU guest. No test-runtime or fixture backend.
set -euo pipefail
[[ -f /etc/lkit-qemu-network ]] || { echo 'Not a QEMU network acceptance guest' >&2; exit 2; }
base=/var/lib/lkit-qemu
journey=$base/journey
mkdir -p "$journey"
chmod 0700 "$journey"

case "${1:-}" in
  identity)
    systemctl is-active --quiet lkit.service
    systemctl is-active --quiet landscape-router.service
    python3 - <<'PY'
import hashlib
import json
from pathlib import Path
root = Path('/srv/landscape')
pin = json.loads(Path('/var/lib/lkit-qemu/landscape-release.json').read_text())
state = json.loads(Path('/root/.lkit/state/install-state.json').read_text())
assert state['active_version'] == pin['version']
assert root.stat().st_dev != Path('/root/.lkit').stat().st_dev, 'installation and territory must use different filesystems'
with (root / 'data/landscape_db.sqlite').open('rb') as database:
    assert database.read(16) == b'SQLite format 3\0', 'real Landscape must create a SQLite database'
with (root / 'current/landscape-webserver').open('rb') as binary:
    assert hashlib.file_digest(binary, 'sha256').hexdigest() == pin['webserver_sha256']
# The original release archive is checked before restore repacks static.zip.
with (root / 'current/static.zip').open('rb') as archive:
    assert hashlib.file_digest(archive, 'sha256').hexdigest() == pin['static_sha256']
print('PASS: pinned official Landscape binary/static, real SQLite and separate installation filesystem')
PY
    ;;
  backup)
    lkit --non-interactive backup create --remark 'QEMU real configuration checkpoint' \
      --output "$journey/original.lkb"
    lkit backup verify --file "$journey/original.lkb"
    python3 - <<'PY'
import filecmp
import json
from pathlib import Path
directory = Path('/var/lib/lkit-qemu/journey')
with (directory / 'original.lkb').open('rb') as stream:
    header = stream.read(32)
    assert header[:4] == b'LKB1'
    metadata = json.loads(stream.read(int.from_bytes(header[6:10], 'little')))
identifier = metadata['backup_id']
local = Path('/root/.lkit/backups') / (identifier + '.lkb')
assert filecmp.cmp(local, directory / 'original.lkb', shallow=False)
assert local.stat().st_mode & 0o077 == 0
assert (directory / 'original.lkb').stat().st_mode & 0o077 == 0
(directory / 'backup-id').write_text(identifier)
print('PASS: manual backup ID and exported external file contain identical private bytes')
PY
    lkit backup verify --backup "$(cat "$journey/backup-id")"
    ;;
  remove-local-backup)
    lkit --non-interactive backup delete --backup "$(cat "$journey/backup-id")" --yes
    [[ ! -e /root/.lkit/backups/$(cat "$journey/backup-id").lkb ]]
    lkit backup verify --file "$journey/original.lkb"
    echo 'PASS: exported backup remains usable after its original catalog entry is deleted'
    ;;
  remove-daemon)
    cp /root/.lkit/state/install-state.json "$journey/state-before-daemon-remove.json"
    chmod 0600 "$journey/state-before-daemon-remove.json"
    lkit self remove
    bash "$0" daemon-absent
    ;;
  daemon-absent)
    [[ -x /usr/local/bin/lkit ]]
    [[ ! -e /etc/systemd/system/lkit.service && ! -L /etc/systemd/system/lkit.service ]]
    [[ ! -e /usr/local/lib/lkit/lkit.service ]]
    if systemctl is-active --quiet lkit.service; then
      echo 'lkit daemon is still active after self remove' >&2
      exit 1
    fi
    systemctl is-active --quiet landscape-router.service
    systemctl is-enabled --quiet landscape-router.service
    lkit network startup-check
    cmp "$journey/state-before-daemon-remove.json" /root/.lkit/state/install-state.json
    echo 'PASS: daemon is removed, CLI startup check succeeds and enabled Landscape retains committed state'
    ;;
  offline)
    wan=$(jq -sr 'map(select(.operation == "install" and .network_takeover != null)) | last | .network_takeover.plan.mode.wan' /root/.lkit/transactions/*.json)
    # QMP returns before the guest necessarily consumes the virtio link event.
    for _ in $(seq 1 10); do
      [[ $(cat "/sys/class/net/$wan/carrier") == 0 ]] && break
      sleep 1
    done
    [[ $(cat "/sys/class/net/$wan/carrier") == 0 ]]
    if curl --fail --silent --connect-timeout 3 --max-time 8 https://api.github.com >/dev/null 2>&1; then
      echo 'Repository remained reachable while the virtual WAN cable was disconnected' >&2
      exit 1
    fi
    echo 'PASS: WAN carrier is down and the official repository is unreachable'
    ;;
  restore-id|restore-file)
    mode=${1#restore-}
    cp /root/.lkit/state/install-state.json "$journey/state-before-$mode.json"
    chmod 0600 "$journey/state-before-$mode.json"
    rm -f "$journey/restore-$mode.exit"
    # Submission survives the temporary loss of LAN SSH while real Landscape restarts.
    nohup bash "$0" "_restore-$mode" </dev/null >"$journey/restore-$mode.log" 2>&1 &
    echo "$!" >"$journey/observer.pid"
    ;;
  _restore-id|_restore-file)
    mode=${1#_restore-}
    set +e
    if [[ $mode == id ]]; then
      lkit --non-interactive restore --backup "$(cat "$journey/backup-id")" --yes
    else
      lkit --non-interactive restore --file "$journey/original.lkb" --yes
    fi
    result=$?
    printf '%s\n' "$result" >"$journey/restore-$mode.exit"
    exit "$result"
    ;;
  pending-id|pending-file|committed)
    if [[ $1 == committed ]]; then
      systemctl is-enabled --quiet lkit.service
      systemctl is-enabled --quiet landscape-router.service
    fi
    python3 - "$1" <<'PY'
import json
from pathlib import Path
import sys
stage = sys.argv[1]
records = [json.loads(path.read_text()) for path in Path('/root/.lkit/transactions').glob('*.json')]
restores = [record for record in records if record['operation'] == 'restore']
assert restores, 'restore transaction is missing'
transaction = max(restores, key=lambda record: (record['started_at'], record['transaction_id']))
assert transaction['schema_version'] == 14, 'restore must use persistent confirmation semantics'
state_path = Path('/root/.lkit/state/install-state.json')
if stage == 'committed':
    assert transaction['phase'] == 'committed'
    assert json.loads(state_path.read_text())['last_transaction_id'] == transaction['transaction_id']
    assert all(record['phase'] in ['committed', 'rolled_back', 'failed'] for record in records)
    print('PASS: explicit confirmation committed the restore state and completed the transaction')
else:
    mode = stage.removeprefix('pending-')
    assert transaction['phase'] == 'awaiting_network_confirmation'
    confirmation = transaction['restore_confirmation']
    assert confirmation['boot_id'] and confirmation['confirmation_deadline'] and confirmation['pending_state']
    assert state_path.read_bytes() == Path(f'/var/lib/lkit-qemu/journey/state-before-{mode}.json').read_bytes(), 'restore committed state before external verification'
    print('PASS: restore awaits external confirmation without committing installation state')
PY
    ;;
  *) echo 'Unknown journey guest stage' >&2; exit 2 ;;
esac
