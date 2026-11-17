#!/usr/bin/env python3
"""Runs only inside test-nspawn-systemd.sh's private-network guest.
Real networkd/systemctl; synthetic interface discovery and local release assets.
"""
import functools
import hashlib
import http.server
import json
import os
from pathlib import Path
import pty
import subprocess
import threading
import time
import zipfile

BASE = Path('/var/lib/lkit-nspawn/networkd-test')
BASE.mkdir(parents=True, exist_ok=True)
RUNTIME = Path('/var/lib/lkit-nspawn/runtime.json')


def run(*args, check=True, **kwargs):
    result = subprocess.run(args, text=True, stdout=subprocess.PIPE,
                            stderr=subprocess.STDOUT, timeout=90, **kwargs)
    if check and result.returncode:
        raise AssertionError(f'{args}: {result.stdout}')
    return result


def ctl(*args, **kwargs):
    return run('systemctl', *args, **kwargs)


def wait(condition):
    deadline = time.monotonic() + 50
    while not condition():
        if time.monotonic() > deadline:
            raise AssertionError('networkd operation timed out')
        time.sleep(.1)


# Fresh installation territory after the native uninstall scenarios.
assert not Path('/root/.lkit/state/install-state.json').exists()
repo = BASE / 'repository'
release = repo / 'releases/1.0.0'
release.mkdir(parents=True, exist_ok=True)
binary = release / 'landscape-webserver-x86_64.zst'
run('zstd', '-q', '-f', '/usr/local/bin/landscape-webserver', '-o', str(binary))
with zipfile.ZipFile(release / 'static.zip', 'w') as archive:
    archive.writestr('static/index.html', '<h1>networkd fixture</h1>')
    archive.writestr('static/lkit-fixture.json', json.dumps({
        'schema_version': 1, 'scenario': 'healthy', 'listen_address': '127.0.0.1',
        'dns_tcp_port': 53, 'dns_udp_port': 53, 'http_port': 6300, 'https_port': 6443,
        'export_version': '1.0.0', 'export_content': 'version = "1.0.0"\n',
    }))


def asset(path):
    return {'url': path.name, 'sha256': hashlib.sha256(path.read_bytes()).hexdigest(),
            'size': path.stat().st_size}


(repo / 'repository.json').write_text('{"protocol_version":1}')
(release / 'manifest.json').write_text(json.dumps({
    'protocol_version': 1, 'version': '1.0.0', 'assets': {
        'webserver': {'x86_64': asset(binary)}, 'static': asset(release / 'static.zip')}}))
server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), functools.partial(
    http.server.SimpleHTTPRequestHandler, directory=str(repo)))
threading.Thread(target=server.serve_forever, daemon=True).start()

sysnet = BASE / 'sys/class/net'
for name, mac in [('ens3', '52:54:00:12:34:01'), ('ens4', '52:54:00:12:34:02')]:
    iface = sysnet / name
    iface.mkdir(parents=True, exist_ok=True)
    for key, value in [('type', '1'), ('address', mac), ('operstate', 'up')]:
        (iface / key).write_text(value + '\n')
ip = BASE / 'ip'
ip.write_text('''#!/usr/bin/python3
import json, sys
args = sys.argv[1:]
if args == ['-j', '-4', 'route', 'show', 'default']:
    print(json.dumps([{'dev':'ens3','gateway':'198.51.100.1'}]))
elif args[:4] == ['-j', '-4', 'addr', 'show']:
    print(json.dumps([{'ifname':'ens3','addr_info':[{'family':'inet','local':'198.51.100.20','prefixlen':24,'scope':'global'}]}, {'ifname':'ens4','addr_info':[]}]))
elif 'flush' in args:
    pass
else:
    print('[]')
''')
ip.chmod(0o755)
runtime = json.loads(RUNTIME.read_text())
runtime.update(sys_class_net=str(sysnet), ip_command=str(ip), network_confirm_timeout_ms=600000)
runtime['health']['stable_duration_ms'] = 300
RUNTIME.write_text(json.dumps(runtime))
# self install intentionally does not persist test-only runtime flags in a service.
# Apply this override only inside the isolated guest for the networkd scenario.
override = Path('/etc/systemd/system/lkit.service.d/networkd-fixture.conf')
override.parent.mkdir(parents=True, exist_ok=True)
override.write_text('[Service]\nExecStart=\nExecStart=/usr/local/bin/lkit daemon --supervise --test-runtime ' + str(RUNTIME) + '\n')
ctl('daemon-reload')
ctl('restart', 'lkit.service')
wait(lambda: Path('/root/.lkit/run/install-v1.pid').exists())
password = BASE / 'password'
password.write_text('Secret123\n')
password.chmod(0o600)
service = 'systemd-networkd.service'
socket = 'systemd-networkd.socket'
waiter = 'systemd-networkd-wait-online.service'
ctl('unmask', service, socket, waiter)
ctl('enable', service)
ctl('disable', socket, waiter)
ctl('mask', '--runtime', waiter)
ctl('start', service)
ctl('stop', socket)
assert ctl('is-active', service).stdout.strip() == 'active'
assert ctl('is-enabled', socket, check=False).stdout.strip() == 'disabled'


def links():
    result = {}
    for directory in ['/etc/systemd/system', '/run/systemd/system']:
        for path in Path(directory).rglob('*'):
            if path.is_symlink() and os.readlink(path) != '/dev/null':
                target = os.readlink(path)
                if 'networkd' in str(path) or 'networkd' in target or 'network-generator' in target:
                    result[str(path)] = target
    return result


before = links()
master, slave = pty.openpty()
env = dict(os.environ, LKIT_TEST_TTY=os.ttyname(slave), LKIT_LANG='en')
os.write(master, b'1\n1\n\n\n\n')
try:
    output = run('/usr/local/bin/lkit', 'install', '--takeover-network', '--version', '1.0.0',
                 '--repository', f'http://127.0.0.1:{server.server_port}',
                 '--install-dir', str(BASE / 'landscape'), '--password-file', str(password),
                 '--test-runtime', str(RUNTIME), env=env)
    print(output.stdout)
finally:
    os.close(master)
    os.close(slave)
for unit in [service, socket, waiter]:
    assert ctl('is-enabled', unit, check=False).stdout.strip() == 'masked', unit
    assert ctl('is-active', unit, check=False).stdout.strip() != 'active', unit
assert links() == before, 'takeover changed startup links'
assert ctl('start', 'dbus-org.freedesktop.network1.service', check=False).returncode != 0
assert ctl('start', socket, check=False).returncode != 0
# Restarting the daemon in the pending window must preserve the same transaction.
ctl('restart', 'lkit.service')
wait(lambda: Path('/root/.lkit/run/network-operations-v1.pid').exists())
run('/usr/local/bin/lkit', 'network', 'rollback', '--test-runtime', str(RUNTIME))
assert ctl('is-active', service).stdout.strip() == 'active'
assert ctl('is-active', socket, check=False).stdout.strip() != 'active'
assert ctl('is-enabled', socket, check=False).stdout.strip() == 'disabled'
assert ctl('is-enabled', waiter, check=False).stdout.strip() == 'masked-runtime'
assert links() == before, 'rollback changed startup links'
assert not Path('/root/.lkit/state/install-state.json').exists()
server.shutdown()
print('PASS: real networkd takeover, daemon restart and exact startup-state restoration')
