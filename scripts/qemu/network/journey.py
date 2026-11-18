#!/usr/bin/env python3
"""Real Landscape v0.28.3 API/data assertions, run by the disposable QEMU job.

The API contract is the official v0.28.3 openapi.json and source, not the lkit
fixture server. Secrets and exported configuration stay in the private test
directory; stdout contains assertion summaries only.
"""

import argparse
from datetime import datetime, timezone
import hashlib
import ipaddress
import json
import math
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
    save(directory / "mutated.json", current)
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


def assert_mutated(api, directory, server):
    saved = json.loads((directory / "mutated.json").read_text())
    current = api.snapshot()
    for section in saved:
        check(current[section] == saved[section], f"rollback did not recover the pre-restore {section}")
    identities = {item["id"] for item in current["redirects"]}
    check(RULE_B not in identities and RULE_LATER in identities, "rollback changed the pre-restore deletion/addition")
    api.assert_web()
    assert_dns(server, DOMAIN_A, "198.51.100.99")
    assert_dns(server, DOMAIN_LATER, "198.51.100.88")
    print("PASS: rollback recovered pre-restore login, frontend, DNS/DHCP/network configuration and external LAN DNS", flush=True)


def verify_backup(directory, filename="original.lkb", snapshot="expected.json"):
    path = directory / filename
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
        saved = json.loads((directory / snapshot).read_text())
        check(export_projection(exported) == saved["export"], "backup omitted or changed real exported configuration")
    print("PASS: exported .lkb checksum, official binary and actual configuration entities verified", flush=True)


FAULT_SCENARIOS = ("explicit", "timeout", "reboot", "powercut")


def timestamp(value):
    return datetime.fromisoformat(value.replace("Z", "+00:00"))


def durable_save(path, value):
    # Persist only our evidence, never modify the production transaction/deadline.
    temporary = path.with_suffix(".tmp")
    with temporary.open("w") as output:
        temporary.chmod(0o600)
        json.dump(value, output, sort_keys=True)
        output.flush()
        os.fsync(output.fileno())
    temporary.replace(path)
    descriptor = os.open(path.parent, os.O_RDONLY | os.O_DIRECTORY)
    try:
        os.fsync(descriptor)
    finally:
        os.close(descriptor)


def startup_links(etc, run):
    links = {}
    managed = (etc / "landscape-router.service").resolve(strict=True)
    for label, base in [("etc", etc), ("run", run)]:
        for path in base.rglob("landscape-router.service"):
            if path.parent.name.endswith((".wants", ".requires")):
                check(path.is_symlink(), "service startup membership is not a symlink")
                resolved = path.resolve(strict=True)
                check(resolved == managed, "startup membership points to a foreign definition")
                links[label + ":" + str(path.relative_to(base))] = {"raw": os.readlink(path), "resolved": str(resolved)}
    return links


def runtime_link_evidence(directory, stage, etc, run):
    current = startup_links(etc, run)
    check(current and all(key.startswith("run:") for key in current), "Landscape must have only runtime startup membership")
    path = directory / "runtime-startup-links.json"
    if stage == "runtime-save":
        durable_save(path, current)
        print("PASS: saved exact runtime-only Landscape startup links", flush=True)
    else:
        check(stage == "runtime-check", "unknown runtime link evidence stage")
        saved = json.loads(path.read_text())
        signature = lambda links: {key: value["resolved"] for key, value in links.items()}
        check(signature(current) == signature(saved), "rollback changed runtime startup members or their managed definition")
        print("PASS: rollback restored runtime-only startup members and equivalent managed targets without persistent membership", flush=True)


def fault_evidence(territory, directory, scenario, stage, boot_id):
    check(scenario in FAULT_SCENARIOS, "unknown restore fault scenario")
    context_path = directory / f"fault-{scenario}.json"
    state_path = territory / "state/install-state.json"
    records = [json.loads(path.read_text()) for path in (territory / "transactions").glob("*.json")]
    if stage == "prepare":
        check(all(record["phase"] in ["committed", "rolled_back", "failed"] for record in records),
              "a previous transaction is unfinished before the fault scenario")
        before = state_path.read_bytes()
        durable_save(context_path, {
            "state": json.loads(before), "state_sha256": hashlib.sha256(before).hexdigest(),
            "known_transactions": [record["transaction_id"] for record in records], "boot_id": boot_id,
        })
        print(f"PASS: saved pre-restore state for {scenario} without changing transaction evidence", flush=True)
        return
    context = json.loads(context_path.read_text())
    if stage == "deadline":
        remaining = (timestamp(context["deadline"]) - datetime.now(timezone.utc)).total_seconds()
        print(max(0, math.ceil(remaining)))
        return
    if stage == "pending":
        candidates = [record for record in records if record["transaction_id"] not in context["known_transactions"]]
        check(len(candidates) == 1, "expected exactly one new restore transaction")
        transaction = candidates[0]
        check(transaction["operation"] == "restore" and transaction["schema_version"] == 14, "fault transaction is not a guarded native restore")
        check(transaction["phase"] == "awaiting_network_confirmation", "restore has not reached external confirmation")
        check(hashlib.sha256(state_path.read_bytes()).hexdigest() == context["state_sha256"], "provisional restore changed committed installation state")
        guard = transaction["restore_confirmation"]
        check(guard["boot_id"] == boot_id == context["boot_id"], "host restarted before fault injection")
        window = (timestamp(guard["confirmation_deadline"]) - timestamp(transaction["updated_at"])).total_seconds()
        check(590 <= window <= 605, "restore is not using the production 600-second confirmation window")
        check((territory / guard["pending_state"]).is_file(), "pending restore state is missing")
        operations = [json.loads(path.read_text()) for path in (territory / "run/operations").glob("*.json")]
        matching = [operation for operation in operations if operation["kind"] == "restore" and operation.get("transaction_id") == transaction["transaction_id"]]
        check(len(matching) == 1 and matching[0]["status"] == "awaiting_confirmation", "restore operation is not awaiting confirmation")
        check(matching[0].get("exit_code") == 0 and not matching[0]["cancellable"], "pending restore operation has an incorrect outcome")
        context.update({"transaction_id": transaction["transaction_id"], "operation_id": matching[0]["id"], "deadline": guard["confirmation_deadline"]})
        durable_save(context_path, context)
        print(f"PASS: {scenario} has an exact pending transaction/operation and unchanged committed state", flush=True)
        return
    check(stage == "rolled-back", "unknown restore fault evidence stage")
    transaction = json.loads((territory / "transactions" / (context["transaction_id"] + ".json")).read_text())
    check(transaction["transaction_id"] == context["transaction_id"] and transaction["phase"] == "rolled_back", "the fault transaction has not rolled back")
    check(transaction["restore_confirmation"]["confirmation_deadline"] == context["deadline"], "fault test transaction deadline changed")
    operation = json.loads((territory / "run/operations" / (context["operation_id"] + ".json")).read_text())
    check(operation["transaction_id"] == context["transaction_id"] and operation["kind"] == "restore", "restore operation identity changed")
    check(operation["status"] == "failed" and operation["stage"] == "finished" and operation["exit_code"] == 5,
          "rolled-back restore operation did not finish with the recovery outcome")
    installed = json.loads(state_path.read_text())
    check(installed["last_transaction_id"] == context["transaction_id"], "rollback state is associated with another transaction")
    stable = lambda state: {key: value for key, value in state.items() if key not in ["last_transaction_id", "committed_at"]}
    check(stable(installed) == stable(context["state"]), "rollback changed committed installation identity/configuration")
    check(all(record["phase"] in ["committed", "rolled_back", "failed"] for record in records), "unfinished transaction remains after rollback")
    if scenario in ["reboot", "powercut"]:
        check(boot_id != context["boot_id"], "fault did not restart the whole VM")
    else:
        check(boot_id == context["boot_id"], "non-reboot rollback unexpectedly restarted the VM")
    if scenario == "timeout":
        check(timestamp(transaction["updated_at"]) >= timestamp(context["deadline"]), "timeout rollback happened before the real deadline")
    elif scenario == "explicit":
        check(timestamp(transaction["updated_at"]) < timestamp(context["deadline"]), "explicit rollback was delayed until expiration")
        operations = [json.loads(path.read_text()) for path in (territory / "run/operations").glob("*.json")]
        rollbacks = [item for item in operations if item["kind"] == "network_rollback" and item.get("network_transaction") == context["transaction_id"]]
        check(len(rollbacks) == 1 and rollbacks[0]["status"] == "succeeded" and rollbacks[0]["exit_code"] == 0,
              "explicit network rollback operation did not succeed")
    print(f"PASS: {scenario} rolled back the exact restore, finalized its operation and recovered prior installation state", flush=True)


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
    parser.add_argument("action", choices=["seed", "mutate", "assert", "assert-mutated", "verify-backup", "verify-mutated-backup", "link-up", "link-down", "fault-state"])
    parser.add_argument("--directory", required=True, type=Path)
    parser.add_argument("--password-file", type=Path)
    parser.add_argument("--address", default="https://192.168.10.1:6443")
    parser.add_argument("--dns-server", default="192.168.10.1")
    parser.add_argument("--qmp", type=Path)
    parser.add_argument("--scenario", choices=FAULT_SCENARIOS)
    parser.add_argument("--stage", choices=["prepare", "pending", "deadline", "rolled-back", "runtime-save", "runtime-check"])
    args = parser.parse_args()
    if args.action.startswith("link-"):
        set_wan_link(args.qmp, args.action == "link-up")
    elif args.action == "verify-backup":
        verify_backup(args.directory)
    elif args.action == "verify-mutated-backup":
        verify_backup(args.directory, "after-faults.lkb", "mutated.json")
    elif args.action == "fault-state":
        check(Path("/etc/lkit-qemu-network").is_file(), "fault evidence requires the disposable QEMU guest")
        if args.stage in ["runtime-save", "runtime-check"]:
            runtime_link_evidence(args.directory, args.stage, Path("/etc/systemd/system"), Path("/run/systemd/system"))
        else:
            fault_evidence(Path("/root/.lkit"), args.directory, args.scenario, args.stage,
                           Path("/proc/sys/kernel/random/boot_id").read_text().strip())
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
        {"seed": seed, "mutate": mutate, "assert": assert_restored, "assert-mutated": assert_mutated}[args.action](api, args.directory, args.dns_server)


if __name__ == "__main__":
    main()
