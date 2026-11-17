"""Temporary-file-only checks for the QEMU archive verifier; no VM or services."""

import contextlib
import hashlib
import io
import json
from pathlib import Path
import tarfile
import tempfile
import unittest
from unittest.mock import patch

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


if __name__ == "__main__":
    unittest.main()
