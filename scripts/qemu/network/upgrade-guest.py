#!/usr/bin/env python3
"""Real old-release installation migration, only in the disposable QEMU guest.

Host stages legacy-lkit-rc3, candidate-lkit and candidate-lkit.sha256 in BASE.
Run prepare before the host drives the old CLI's install and network confirm.
Then run move-to-disk and upgrade; after the host reboots, assert-rebooted.
Long actions may run under nohup; upgrade/<action>.exit records their outcome.
Published rc3 -> rc4 uses the unmodified old CLI's real self upgrade. The
unpublished candidate is a separate, explicitly labelled atomic deployment.
"""
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import stat
import subprocess
import sys
import tempfile
import time

BASE = Path('/var/lib/lkit-qemu')
WORK = BASE / 'upgrade'
ROOT = Path('/srv/landscape')
STATE = Path('/root/.lkit/state/install-state.json')
CLI = Path('/usr/local/bin/lkit')
ORIGIN = ROOT / 'service/landscape-router.service'
UNIT = 'landscape-router.service'
PIN = json.loads(Path(__file__).with_name('lkit-upgrade-releases.json').read_text())


def check(condition, message):
    if not condition:
        raise AssertionError(message)


def run(*args):
    return subprocess.check_output(args, text=True).strip()


def command(*args):
    subprocess.run(args, check=True)


def digest(path):
    with Path(path).open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def save(name, value):
    (WORK / name).write_text(json.dumps(value, sort_keys=True))


def saved(name):
    return json.loads((WORK / name).read_text())


def verify_release(path, release):
    check(path.stat().st_size == release['size'], 'published lkit size differs from pin')
    check(digest(path) == release['sha256'], 'published lkit checksum differs from pin')
    check(run(str(path), '--version').split() == ['lkit', release['version']],
          'published lkit version differs from pin')


def candidate_digest():
    expected = (BASE / 'candidate-lkit.sha256').read_text().strip()
    check(re.fullmatch('[0-9a-f]{64}', expected), 'invalid candidate digest manifest')
    check(digest(BASE / 'candidate-lkit') == expected, 'candidate transport checksum mismatch')
    return expected


def deploy(source):
    # Installation-local temporary file, executable self-check, atomic publish.
    descriptor, temporary = tempfile.mkstemp(prefix='.lkit.qemu-upgrade.', dir=CLI.parent)
    try:
        with os.fdopen(descriptor, 'wb') as output, source.open('rb') as input_stream:
            shutil.copyfileobj(input_stream, output)
            os.fchmod(output.fileno(), 0o755)
            output.flush()
            os.fsync(output.fileno())
        check(run(temporary, '--version').startswith('lkit '), 'staged lkit self-check failed')
        os.replace(temporary, CLI)
        directory = os.open(CLI.parent, os.O_RDONLY | os.O_DIRECTORY)
        try:
            os.fsync(directory)
        finally:
            os.close(directory)
    finally:
        Path(temporary).unlink(missing_ok=True)


def service():
    facts = dict(line.split('=', 1) for line in run(
        'systemctl', 'show', UNIT, '--property=MainPID,ActiveState,UnitFileState').splitlines())
    pid = int(facts['MainPID'])
    # Pair PID with process start time so reuse cannot hide an unexpected restart.
    facts['start_ticks'] = (Path(f'/proc/{pid}/stat').read_text().rsplit(')', 1)[1].split()[19]
                            if pid else None)
    return facts


def assert_service(expected):
    check(service() == expected, 'repair or lkit upgrade changed Landscape PID/enabled/active state')


def await_candidate_daemon(expected):
    deadline = time.monotonic() + 60
    pidfile = Path('/root/.lkit/run/lkit.pid')
    capability = Path('/root/.lkit/run/service-repair-v1.pid')
    while True:
        supervisor = dict(line.split('=', 1) for line in run(
            'systemctl', 'show', 'lkit.service', '--property=MainPID,ActiveState').splitlines())
        pid = pidfile.read_text().strip() if pidfile.exists() else ''
        executable = Path(f'/proc/{pid}/exe')
        if (supervisor['ActiveState'] == 'active' and supervisor['MainPID'] != '0'
                and pid.isdecimal() and int(pid) > 0 and executable.is_file()
                and capability.exists() and capability.read_text().strip() == pid):
            check(digest(executable) == expected, 'resident daemon is not the candidate')
            return
        check(time.monotonic() < deadline, 'candidate daemon never advertised service repair')
        time.sleep(0.2)


def assert_terminal():
    state = json.loads(STATE.read_text())
    transactions = [json.loads(path.read_text())
                    for path in Path('/root/.lkit/transactions').glob('*.json')]
    check(all(tx['phase'] in ('committed', 'rolled_back', 'failed') for tx in transactions),
          'unfinished transaction remains in the old installation')
    check(any(tx['transaction_id'] == state['last_transaction_id'] and tx['phase'] == 'committed'
              for tx in transactions), 'installation state lacks its committed transaction')


def tree_identity(directory):
    entries = {}
    for path in sorted(directory.rglob('*')):
        metadata = path.lstat()
        check(not stat.S_ISSOCK(metadata.st_mode), f'unexpected socket in stopped installation: {path}')
        content = (os.readlink(path) if path.is_symlink()
                   else digest(path) if path.is_file() else None)
        entries[str(path.relative_to(directory))] = [metadata.st_mode, metadata.st_uid,
                                                     metadata.st_gid, content]
    return entries


def assert_stopped_services():
    for unit in (UNIT, 'lkit.service'):
        facts = dict(line.split('=', 1) for line in run(
            'systemctl', 'show', unit, '--property=MainPID,ActiveState').splitlines())
        check(facts['MainPID'] == '0' and facts['ActiveState'] == 'inactive',
              f'{unit} did not stop before disk migration')


def unix_socket_paths(table):
    lines = table.splitlines()
    check(lines and lines[0].split() == ['Num', 'RefCount', 'Protocol', 'Flags', 'Type',
                                        'St', 'Inode', 'Path'], 'invalid /proc/net/unix header')
    paths = set()
    for line in lines[1:]:
        fields = line.split(maxsplit=7)
        check(len(fields) in (7, 8), 'invalid /proc/net/unix row')
        if len(fields) == 8:
            paths.add(fields[7])
    return paths


def remove_stale_registration_socket(root, table):
    # Landscape 0.28.3 leaves this runtime socket after a clean stop and removes
    # it before listening again. Do not copy it or remove any other user file.
    socket = root / 'data/unix_link/register.sock'
    for directory in reversed(socket.parents):
        check(stat.S_ISDIR(directory.lstat().st_mode),
              f'runtime socket ancestor is not a real directory: {directory}')
    try:
        metadata = socket.lstat()
    except FileNotFoundError:
        return
    check(stat.S_ISSOCK(metadata.st_mode), f'known runtime socket is not a socket: {socket}')
    check(str(socket) not in unix_socket_paths(table), f'runtime socket is still held: {socket}')
    # Preflight the entire /srv migration tree before removing even this known
    # runtime entry. An unrelated socket must leave every existing file intact.
    for path in sorted(root.parent.rglob('*')):
        check(not stat.S_ISSOCK(path.lstat().st_mode) or path == socket,
              f'unexpected socket in stopped installation: {path}')
    socket.unlink()
    print('PASS: removed only the stopped Landscape registration socket after checking /proc/net/unix', flush=True)


def prepare():
    check(not STATE.exists(), 'prepare requires an uninstalled VM')
    check(not Path('/etc/systemd/system/lkit.service').exists(), 'lkit is already installed')
    check(set(path.name for path in Path('/srv').iterdir()) <= {'lost+found'},
          '/srv must be empty before the old installation')
    check(run('findmnt', '-n', '-o', 'SOURCE', '--mountpoint', '/srv') == '/dev/vdb',
          '/srv is not the disposable second disk')
    verify_release(BASE / 'legacy-lkit-rc3', PIN['source'])
    candidate_digest()
    command('umount', '/srv')
    check(Path('/srv').stat().st_dev == Path('/root').stat().st_dev,
          'old release must install on one filesystem because of its EXDEV limitation')
    deploy(BASE / 'legacy-lkit-rc3')
    verify_release(CLI, PIN['source'])
    print('PASS: authentic published rc3 staged; old install starts with /srv on the system disk', flush=True)


def move_to_disk():
    verify_release(CLI, PIN['source'])
    assert_terminal()
    check(os.readlink('/etc/systemd/system/' + UNIT) == str(ORIGIN),
          'old release did not create its original installation-local unit registration')
    check(ROOT.stat().st_dev == STATE.stat().st_dev, 'old install was not on the system disk')
    command('systemctl', 'stop', UNIT, 'lkit.service')
    assert_stopped_services()
    remove_stale_registration_socket(ROOT, Path('/proc/net/unix').read_text())
    source = tree_identity(Path('/srv'))
    mount = WORK / 'installation-disk'
    mount.mkdir()
    command('mount', '/dev/vdb', str(mount))
    # ext4's empty lost+found is filesystem bookkeeping, not user installation data.
    check(set(path.name for path in mount.iterdir()) <= {'lost+found'}, 'second disk is not empty')
    command('cp', '-a', '/srv/.', str(mount) + '/')
    copied = tree_identity(mount)
    copied.pop('lost+found', None)
    check(copied == source, 'disk migration changed installation contents, links or ownership')
    command('sync', '-f', str(mount))
    command('umount', str(mount))
    command('mount', '/dev/vdb', '/srv')
    check(ROOT.stat().st_dev != STATE.stat().st_dev, 'installation is not on the independent disk')
    command('systemctl', 'start', 'lkit.service', UNIT)
    facts = service()
    check(facts['ActiveState'] == 'active' and facts['UnitFileState'] == 'enabled'
          and facts['MainPID'] != '0', 'old installation did not restart after disk migration')
    save('old-service.json', facts)
    save('old-state.json', json.loads(STATE.read_text()))
    save('old-definition-sha256.json', digest(ORIGIN))
    print('PASS: authentic old deployment copied with ownership intact to independent /srv; no old-version reboot', flush=True)


def assert_boot_copy():
    copy = Path('/etc/systemd/lkit-managed') / hashlib.sha256(str(ORIGIN).encode()).hexdigest() / UNIT
    check(os.readlink('/etc/systemd/system/' + UNIT) == str(copy), 'registration is not the boot copy')
    header, body = copy.read_text().split('\n', 1)
    prefix = '# lkit managed systemd definition v1 '
    check(header.startswith(prefix), 'boot copy identity header missing')
    identity = json.loads(header[len(prefix):])
    check(identity == {'origin': str(ORIGIN), 'sha256': digest(ORIGIN)}, 'boot copy identity mismatch')
    source = ORIGIN.read_text()
    check(body == source + '\n[Unit]\nRequiresMountsFor="/srv/landscape"\n',
          'boot copy content is not the verified original plus mount requirement')
    check('network startup-check' in source, 'repaired definition lacks startup guard')
    check(Path(run('systemctl', 'show', UNIT, '--property=FragmentPath', '--value')).resolve() == copy,
          'PID 1 has not loaded the boot-readable definition')


def upgrade():
    verify_release(CLI, PIN['source'])
    before = saved('old-service.json')
    assert_service(before)
    # Uses real GitHub metadata, SHA256SUMS and published binary. No API remapping.
    command(str(CLI), 'self', 'upgrade', '--version', PIN['released_target']['tag'])
    verify_release(CLI, PIN['released_target'])
    assert_service(before)
    check(digest(ORIGIN) == saved('old-definition-sha256.json'), 'self upgrade rewrote Landscape definition')
    print('PASS: unmodified rc3 self upgrade downloaded and installed the real published rc4', flush=True)

    expected = candidate_digest()
    deploy(BASE / 'candidate-lkit')
    check(digest(CLI) == expected, 'candidate publication checksum mismatch')
    command(str(CLI), '--non-interactive', 'self', 'install')
    await_candidate_daemon(expected)
    assert_service(before)
    print('PASS: unpublished candidate deployed separately by atomic replacement and self install', flush=True)

    command(str(CLI), '--non-interactive', 'repair', 'service')
    assert_service(before)
    assert_boot_copy()
    after = json.loads(STATE.read_text())
    old = saved('old-state.json')
    for field in ('schema_version', 'layout_version', 'install_root', 'canonical_install_root',
                  'active_version', 'assets', 'initialization'):
        check(after[field] == old[field], f'service repair changed installation field {field}')
    assert_terminal()
    print('PASS: candidate repaired genuine old service registration without restarting or enabling Landscape', flush=True)

    # This is an explicit user stop/disable followed by an idempotent repair.
    copy = os.readlink('/etc/systemd/system/' + UNIT)
    command('systemctl', 'disable', '--now', UNIT)
    # systemctl disable can also unlink a linked unit's primary registration.
    # Keep the definition registered, without creating any startup membership.
    command('systemctl', 'link', copy)
    stopped = service()
    check(stopped['MainPID'] == '0' and stopped['ActiveState'] == 'inactive'
          and stopped['UnitFileState'] in ('disabled', 'linked'), 'manual stop/disable did not take effect')
    state_before = STATE.read_bytes()
    command(str(CLI), '--non-interactive', 'repair', 'service')
    assert_service(stopped)
    check(STATE.read_bytes() == state_before, 'idempotent stopped-service repair changed install state')
    print('PASS: repair preserves an explicitly disabled and stopped Landscape', flush=True)
    command('systemctl', 'enable', UNIT)
    command('systemctl', 'start', UNIT)
    assert_boot_copy()
    save('pre-reboot-boot-id.json', Path('/proc/sys/kernel/random/boot_id').read_text().strip())
    print('PASS: Landscape explicitly re-enabled and started for the host-driven reboot', flush=True)


def assert_rebooted():
    check(Path('/proc/sys/kernel/random/boot_id').read_text().strip() != saved('pre-reboot-boot-id.json'),
          'guest did not reboot')
    check(ROOT.stat().st_dev != STATE.stat().st_dev, 'reboot lost the independent installation mount')
    check(digest(CLI) == candidate_digest(), 'reboot did not retain the candidate binary')
    for unit in ('lkit.service', UNIT):
        command('systemctl', 'is-active', '--quiet', unit)
        command('systemctl', 'is-enabled', '--quiet', unit)
    await_candidate_daemon(candidate_digest())
    command(str(CLI), 'network', 'startup-check')
    assert_boot_copy()
    assert_terminal()
    print('PASS: candidate and repaired old installation start through real systemd after a full reboot', flush=True)


def main():
    check(os.geteuid() == 0 and Path('/etc/lkit-qemu-network').is_file()
          and Path('/etc/hostname').read_text().strip() == 'lkit-qemu',
          'requires the disposable QEMU guest')
    check(not os.environ.get('LKIT_TERRITORY') and not os.environ.get('LKIT_GLOBAL_DIR'),
          'upgrade acceptance must use production paths')
    credentials = BASE / 'github.env'
    if credentials.is_file():
        for line in credentials.read_text().splitlines():
            if line.startswith('GITHUB_TOKEN='):
                os.environ['GITHUB_TOKEN'] = line.removeprefix('GITHUB_TOKEN=')
    actions = {'prepare': prepare, 'move-to-disk': move_to_disk, 'upgrade': upgrade,
               'assert-rebooted': assert_rebooted}
    check(len(sys.argv) == 2 and sys.argv[1] in actions, 'unknown upgrade acceptance stage')
    WORK.mkdir(mode=0o700, exist_ok=True)
    action = sys.argv[1]
    result = 1
    try:
        actions[action]()
        result = 0
    finally:
        (WORK / f'{action}.exit').write_text(str(result) + '\n')


if __name__ == '__main__':
    main()
