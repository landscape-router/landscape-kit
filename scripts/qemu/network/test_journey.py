"""Temporary-file-only checks for the QEMU archive verifier; no VM or services."""

import contextlib
import hashlib
import io
import json
from pathlib import Path
import tarfile
import tempfile
import unittest
from unittest.mock import Mock, patch

import journey


class BackupArchiveTests(unittest.TestCase):
    def test_embedded_gzip_is_verified_from_payload_offset_without_rewinding(self):
        binary = b"official-binary-test-content\0" * 100000
        exported = f'version = "{journey.VERSION}"\n[[dns_redirects]]\nremark = "archive assertion"\n'
        payload = io.BytesIO()
        with tarfile.open(fileobj=payload, mode="w:gz") as archive:
            for name, content in [("landscape-webserver", binary), ("landscape_init.toml", exported.encode())]:
                member = tarfile.TarInfo(name)
                member.size = len(content)
                archive.addfile(member, io.BytesIO(content))
        compressed = payload.getvalue()
        metadata = json.dumps({
            "landscape_version": journey.VERSION, "architecture": "x86_64",
            "auto": False, "scope": "minimal",
            "checksum": "sha256:" + hashlib.sha256(compressed).hexdigest(),
        }).encode()
        header = b"LKB1" + (1).to_bytes(2, "little") + len(metadata).to_bytes(4, "little") + bytes(22)
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            (directory / "original.lkb").write_bytes(
                (header + metadata).ljust(1024 * 1024, b"\0") + compressed
            )
            (directory / "expected.json").write_text(json.dumps({"export": journey.export_projection(exported)}))
            with patch.object(journey, "BINARY_SHA256", hashlib.sha256(binary).hexdigest()):
                with contextlib.redirect_stdout(io.StringIO()):
                    journey.verify_backup(directory)


class ApiMutationTests(unittest.TestCase):
    def test_edits_echo_current_server_revisions_instead_of_normalized_snapshot(self):
        api = Mock(spec=journey.Api)
        current_rule = journey.rule(journey.RULE_A, journey.DOMAIN_A, "198.51.100.41")
        current_rule["update_at"] = 17.125
        current_dhcp = {"iface_name": "br_lan", "enable": True, "update_at": 29.5, "config": {"address_lease_time": 1800}}
        reads = {
            "/api/v1/dns/redirects/" + journey.RULE_A: current_rule,
            "/api/v1/services/dhcp_v4/br_lan": current_dhcp,
        }
        api.call.side_effect = lambda method, path, payload=None: reads[path] if method == "GET" else None
        api.snapshot.return_value = {
            "redirects": [{"id": journey.RULE_A}, {"id": journey.RULE_LATER}],
            "dhcp": {"config": {"address_lease_time": 600}}, "export": {"changed": True},
        }
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            # The saved comparison state intentionally has no update_at revision.
            (directory / "expected.json").write_text(json.dumps({
                "dhcp": journey.normalized(current_dhcp), "export": {},
            }))
            with patch.object(journey, "assert_dns"), contextlib.redirect_stdout(io.StringIO()):
                journey.mutate(api, directory, "192.0.2.1")
        edits = [call.args[2] for call in api.call.call_args_list if call.args[0] in ["POST", "PUT"]]
        self.assertEqual(edits[0]["update_at"], 17.125)
        self.assertEqual(edits[0]["result_info"], ["198.51.100.99"])
        self.assertEqual(edits[-1]["update_at"], 29.5)
        self.assertEqual(edits[-1]["config"]["address_lease_time"], 600)


if __name__ == "__main__":
    unittest.main()
