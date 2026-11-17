#!/usr/bin/env python3
"""Real Landscape v0.28.3 API/data assertions, run by the disposable QEMU job.

The API contract is the official v0.28.3 openapi.json and source, not the lkit
fixture server. Secrets and exported configuration stay in the private test
directory; stdout contains assertion summaries only.
"""

import argparse
import hashlib
import ipaddress
import json
import os
from pathlib import Path
import socket
import ssl
import struct
import tarfile
import time
import tomllib
import urllib.error
import urllib.request

PIN = json.loads(Path(__file__).with_name("landscape-release.json").read_text())
VERSION = PIN["version"]
BINARY_SHA256 = PIN["webserver_sha256"]
RULE_A = "1054f7b9-625f-4e58-8241-77ea40226401"
RULE_B = "1054f7b9-625f-4e58-8241-77ea40226402"
RULE_LATER = "1054f7b9-625f-4e58-8241-77ea40226403"
DOMAIN_A = "restored-a.lkit-journey.test"
DOMAIN_B = "restored-b.lkit-journey.test"
DOMAIN_LATER = "later.lkit-journey.test"
EXPORT_SECTIONS = (
    "version", "ifaces", "marks", "wan_links", "route_lans", "route_wans",
    "dns_redirects", "dns_rules", "dns_upstream_configs", "dhcpv4_services",
)


def check(condition, message):
    if not condition:
        raise AssertionError(message)


def normalized(value):
    if isinstance(value, dict):
        return {key: normalized(item) for key, item in value.items() if key != "update_at"}
    if isinstance(value, list):
        return sorted((normalized(item) for item in value), key=lambda item: json.dumps(item, sort_keys=True))
    return value


def export_projection(content):
    value = tomllib.loads(content)
    check(value.get("version") == VERSION, "exported configuration version differs from the pinned release")
    return normalized({key: value.get(key, []) for key in EXPORT_SECTIONS})


class Api:
    def __init__(self, address, password_file):
        self.address = address.rstrip("/")
        self.token = None
        context = ssl.SSLContext(ssl.PROTOCOL_TLS_CLIENT)
        context.check_hostname = False
        context.verify_mode = ssl.CERT_NONE  # The disposable router generates its own certificate.
        self.opener = urllib.request.build_opener(
            urllib.request.ProxyHandler({}), urllib.request.HTTPSHandler(context=context)
        )
        password = password_file.read_text().rstrip("\n")
        login = self.call("POST", "/api/auth/login", {"username": "admin", "password": password})
        check(login.get("success") is True and login.get("token"), "real administrator login failed")
        self.token = login["token"]

    def call(self, method, path, payload=None):
        headers = {"Accept": "application/json"}
        if self.token:
            headers["Authorization"] = "Bearer " + self.token
        body = None if payload is None else json.dumps(payload).encode()
        if body is not None:
            headers["Content-Type"] = "application/json"
        request = urllib.request.Request(self.address + path, body, headers, method=method)
        try:
            with self.opener.open(request, timeout=15) as response:
                result = json.load(response)
        except urllib.error.HTTPError as error:
            # Never print response content: exported config and credentials are sensitive.
            raise AssertionError(f"{method} {path} returned HTTP {error.code}") from None
        check(not result.get("error_id"), f"{method} {path} returned a business error")
        check("data" in result, f"{method} {path} omitted its result")
        return result["data"]

    def assert_web(self):
        with self.opener.open(self.address + "/", timeout=15) as response:
            page = response.read(65536).lower()
        check(b"<html" in page, "real frontend is not served over LAN HTTPS")

    def snapshot(self):
        exported = self.call("GET", "/api/v1/system/config/export")
        check(exported["version"] == VERSION, "export API returned an unexpected version")
        return {
            "redirects": normalized(self.call("GET", "/api/v1/dns/redirects")),
            "dhcp": normalized(self.call("GET", "/api/v1/services/dhcp_v4/br_lan")),
            "export": export_projection(exported["content"]),
        }


def rule(identity, domain, address):
    return {
        "id": identity, "name": domain, "remark": "lkit real backup journey",
        "enable": True, "match_rules": [{"t": "config", "match_type": "full", "value": domain}],
        "answer_mode": "static_ips", "result_info": [address], "apply_flows": [],
        "block_metadata_queries": True,
    }


def skip_name(message, offset):
    for _ in range(128):
        check(offset < len(message), "truncated DNS name")
        length = message[offset]
        if length & 0xC0 == 0xC0:
            check(offset + 1 < len(message), "truncated DNS compression pointer")
            return offset + 2
        check(length < 64, "invalid DNS label")
        offset += 1
        if length == 0:
            return offset
        offset += length
    raise AssertionError("DNS name is too long")


def dns_answers(server, domain):
    identity = int.from_bytes(os.urandom(2), "big")
    question = b"".join(bytes([len(label)]) + label.encode() for label in domain.split(".")) + b"\0"
    query = struct.pack("!6H", identity, 0x0100, 1, 0, 0, 0) + question + struct.pack("!HH", 1, 1)
    with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as client:
        client.settimeout(3)
        client.connect((server, 53))
        client.send(query)
        answer = client.recv(4096)
    check(len(answer) >= 12, "truncated DNS response")
    reply_id, flags, questions, count, _, _ = struct.unpack_from("!6H", answer)
    check(reply_id == identity and flags & 0x8000, "DNS response identity mismatch")
    check(flags & 0xF == 0, "DNS response is not successful")
    offset = 12
    for _ in range(questions):
        offset = skip_name(answer, offset) + 4
    addresses = set()
    for _ in range(count):
        offset = skip_name(answer, offset)
        kind, family, _, length = struct.unpack_from("!HHIH", answer, offset)
        offset += 10
        check(offset + length <= len(answer), "truncated DNS answer")
        if kind == 1 and family == 1 and length == 4:
            addresses.add(str(ipaddress.IPv4Address(answer[offset:offset + length])))
        offset += length
    return addresses


def assert_dns(server, domain, address):
    deadline = time.monotonic() + 45
    while time.monotonic() < deadline:
        try:
            if dns_answers(server, domain) == {address}:
                print(f"PASS: external LAN DNS {domain} = {address}", flush=True)
                return
        except (OSError, AssertionError, struct.error):
            pass
        time.sleep(1)
    raise AssertionError(f"external LAN DNS did not return the expected address for {domain}")


def save(path, value):
    path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    with path.open("w") as output:
        path.chmod(0o600)
        json.dump(value, output, sort_keys=True)


def seed(api, directory, server):
    api.assert_web()
    for identity, domain, address in [(RULE_A, DOMAIN_A, "198.51.100.41"), (RULE_B, DOMAIN_B, "198.51.100.42")]:
        created = api.call("POST", "/api/v1/dns/redirects", rule(identity, domain, address))
        check(created["id"] == identity, "DNS create did not retain the submitted identity")
    dhcp = api.call("GET", "/api/v1/services/dhcp_v4/br_lan")
    check(dhcp["enable"] is True, "installation did not enable the real DHCP service")
    dhcp["config"].update({
        "ip_range_start": "192.168.10.140", "ip_range_end": "192.168.10.160",
        "address_lease_time": 1800, "custom_options": [{"TFTPServerName": "boot.lkit-journey.test"}],
    })
    api.call("PUT", "/api/v1/services/dhcp_v4", dhcp)
    saved = api.snapshot()
    check(saved["export"].get("dns_redirects") and saved["export"].get("dhcpv4_services"),
          "real export omitted configured DNS or DHCP entities")
    save(directory / "expected.json", saved)
    assert_dns(server, DOMAIN_A, "198.51.100.41")
    assert_dns(server, DOMAIN_B, "198.51.100.42")
    print("PASS: real login, frontend, DNS and DHCP configuration created and exported", flush=True)


def mutate(api, directory, server):
    saved = json.loads((directory / "expected.json").read_text())
    # Official API edits require the latest update_at optimistic-lock revision.
    # Normalized backup snapshots deliberately omit it and are comparison-only.
    changed_rule = api.call("GET", "/api/v1/dns/redirects/" + RULE_A)
    changed_rule["result_info"] = ["198.51.100.99"]
    api.call("POST", "/api/v1/dns/redirects", changed_rule)
    api.call("DELETE", "/api/v1/dns/redirects/" + RULE_B)
    api.call("POST", "/api/v1/dns/redirects", rule(RULE_LATER, DOMAIN_LATER, "198.51.100.88"))
    dhcp = api.call("GET", "/api/v1/services/dhcp_v4/br_lan")
    dhcp["config"].update({"address_lease_time": 600, "custom_options": []})
    api.call("PUT", "/api/v1/services/dhcp_v4", dhcp)
    current = api.snapshot()
    identities = {item["id"] for item in current["redirects"]}
    check(RULE_B not in identities and RULE_LATER in identities, "delete/add mutation did not reach the real database")
    check(current["dhcp"] != saved["dhcp"], "DHCP mutation did not persist")
    check(current["export"] != saved["export"], "real export did not observe configuration mutations")
    assert_dns(server, DOMAIN_A, "198.51.100.99")
    assert_dns(server, DOMAIN_LATER, "198.51.100.88")
    print("PASS: pre-restore state has changed, deleted and newly added real configuration", flush=True)


def assert_restored(api, directory, server):
    saved = json.loads((directory / "expected.json").read_text())
    current = api.snapshot()
    for section in saved:
        check(current[section] == saved[section], f"restored {section} differs semantically from the backup-time configuration")
    check(RULE_LATER not in {item["id"] for item in current["redirects"]}, "post-backup DNS record survived restore")
    api.assert_web()
    assert_dns(server, DOMAIN_A, "198.51.100.41")
    assert_dns(server, DOMAIN_B, "198.51.100.42")
    print("PASS: real login, frontend, exported network/DNS/DHCP configuration and LAN DNS restored", flush=True)


def verify_backup(directory):
    path = directory / "original.lkb"
    with path.open("rb") as stream:
        header = stream.read(32)
        check(header[:4] == b"LKB1" and int.from_bytes(header[4:6], "little") == 1, "unexpected backup format")
        metadata = json.loads(stream.read(int.from_bytes(header[6:10], "little")))
        check(metadata["landscape_version"] == VERSION and metadata["architecture"] == "x86_64", "backup release identity differs")
        check(metadata["auto"] is False and metadata["scope"] == "minimal", "backup is not the requested manual snapshot")
        stream.seek(1024 * 1024)
        digest = hashlib.file_digest(stream, "sha256").hexdigest()
        check(metadata["checksum"] == "sha256:" + digest, "backup checksum differs")
        stream.seek(1024 * 1024)
        # gzip random access would rewind the enclosing .lkb to byte zero,
        # where its metadata header (rather than gzip) lives. Read the embedded
        # archive once from the payload offset, without extracting private data.
        names = set()
        binary_digest = None
        exported = None
        with tarfile.open(fileobj=stream, mode="r|gz") as archive:
            for member in archive:
                name = member.name.removeprefix("./")
                names.add(name)
                if name == "landscape-webserver":
                    binary_digest = hashlib.file_digest(archive.extractfile(member), "sha256").hexdigest()
                elif name == "landscape_init.toml":
                    exported = archive.extractfile(member).read().decode()
        check("landscape_db.sqlite" not in names and "landscape_api_token" not in names, "backup contains runtime-only data")
        check(binary_digest == BINARY_SHA256, "backup does not contain the pinned production binary")
        check(exported is not None, "backup omitted exported configuration")
        saved = json.loads((directory / "expected.json").read_text())
        check(export_projection(exported) == saved["export"], "backup omitted or changed real exported configuration")
    print("PASS: exported .lkb checksum, official binary and actual configuration entities verified", flush=True)


def set_wan_link(path, up):
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as monitor:
        monitor.settimeout(10)
        monitor.connect(str(path))
        stream = monitor.makefile("rwb", buffering=0)
        check("QMP" in json.loads(stream.readline()), "QEMU monitor greeting is missing")
        for command in [{"execute": "qmp_capabilities"}, {"execute": "set_link", "arguments": {"name": "wan", "up": up}}]:
            stream.write(json.dumps(command).encode() + b"\n")
            while True:
                reply = json.loads(stream.readline())
                if "event" not in reply:
                    check("return" in reply and "error" not in reply, "QEMU rejected WAN link control")
                    break
    print("PASS: QEMU WAN link " + ("connected" if up else "disconnected"), flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=["seed", "mutate", "assert", "verify-backup", "link-up", "link-down"])
    parser.add_argument("--directory", required=True, type=Path)
    parser.add_argument("--password-file", type=Path)
    parser.add_argument("--address", default="https://192.168.10.1:6443")
    parser.add_argument("--dns-server", default="192.168.10.1")
    parser.add_argument("--qmp", type=Path)
    args = parser.parse_args()
    if args.action.startswith("link-"):
        set_wan_link(args.qmp, args.action == "link-up")
    elif args.action == "verify-backup":
        verify_backup(args.directory)
    else:
        # A native service becomes active before its HTTPS listener is ready.
        # Retry transport readiness, while rejecting authentication/API failures.
        deadline = time.monotonic() + 90
        while True:
            try:
                api = Api(args.address, args.password_file)
                break
            except (OSError, urllib.error.URLError):
                if time.monotonic() >= deadline:
                    raise AssertionError("real Landscape login endpoint did not become reachable") from None
                time.sleep(2)
        {"seed": seed, "mutate": mutate, "assert": assert_restored}[args.action](api, args.directory, args.dns_server)


if __name__ == "__main__":
    main()
