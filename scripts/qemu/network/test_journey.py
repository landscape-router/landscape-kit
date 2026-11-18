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

    def test_rollback_assertion_requires_pre_restore_data_not_backup_data(self):
        api = Mock(spec=journey.Api)
        mutated = {"redirects": [{"id": journey.RULE_A}, {"id": journey.RULE_LATER}],
                   "dhcp": {"lease": 600}, "export": {"changed": True}}
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            (directory / "mutated.json").write_text(json.dumps(mutated))
            api.snapshot.return_value = mutated
            with patch.object(journey, "assert_dns") as dns, contextlib.redirect_stdout(io.StringIO()):
                journey.assert_mutated(api, directory, "192.0.2.1")
            self.assertEqual(dns.call_args_list[0].args[-1], "198.51.100.99")
            self.assertEqual(dns.call_args_list[1].args[-1], "198.51.100.88")
            api.snapshot.return_value = {**mutated, "dhcp": {"lease": 1800}}
            with self.assertRaisesRegex(AssertionError, "pre-restore dhcp"):
                journey.assert_mutated(api, directory, "192.0.2.1")


class FaultEvidenceTests(unittest.TestCase):
    def fixture(self, scenario):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        root = Path(temporary.name)
        territory, directory = root / "territory", root / "journey"
        directory.mkdir()
        for path in ["transactions", "state", "run/operations"]:
            (territory / path).mkdir(parents=True, exist_ok=True)
        original = {"active_version": journey.VERSION, "assets": {"sha": "old"},
                    "last_transaction_id": "before", "committed_at": "before"}
        self.write(territory / "state/install-state.json", original)
        self.evidence(territory, directory, scenario, "prepare", "boot-one")
        transaction = {
            "transaction_id": "restore-one", "operation": "restore", "schema_version": 14,
            "phase": "awaiting_network_confirmation", "updated_at": "2026-11-17T12:00:00Z",
            "restore_confirmation": {"boot_id": "boot-one", "confirmation_deadline": "2026-11-17T12:10:00Z",
                                     "pending_state": "transactions/restore-one/pending.json"},
        }
        pending = territory / transaction["restore_confirmation"]["pending_state"]
        pending.parent.mkdir()
        pending.write_text("{}")
        self.write(territory / "transactions/restore-one.json", transaction)
        operation = {"id": "operation-one", "kind": "restore", "transaction_id": "restore-one",
                     "status": "awaiting_confirmation", "exit_code": 0, "cancellable": False}
        self.write(territory / "run/operations/operation-one.json", operation)
        self.evidence(territory, directory, scenario, "pending", "boot-one")
        return territory, directory, original, transaction, operation

    @staticmethod
    def write(path, value):
        path.write_text(json.dumps(value))

    @staticmethod
    def evidence(*args):
        with contextlib.redirect_stdout(io.StringIO()):
            journey.fault_evidence(*args)

    def finish(self, scenario):
        territory, directory, original, transaction, operation = self.fixture(scenario)
        transaction.update({"phase": "rolled_back", "updated_at": "2026-11-17T12:11:00Z" if scenario == "timeout" else "2026-11-17T12:01:00Z"})
        operation.update({"status": "failed", "stage": "finished", "exit_code": 5})
        self.write(territory / "transactions/restore-one.json", transaction)
        self.write(territory / "run/operations/operation-one.json", operation)
        self.write(territory / "state/install-state.json", {**original, "last_transaction_id": "restore-one", "committed_at": "after"})
        if scenario == "explicit":
            self.write(territory / "run/operations/rollback-one.json", {
                "kind": "network_rollback", "network_transaction": "restore-one", "status": "succeeded", "exit_code": 0,
            })
        boot = "boot-two" if scenario in ["reboot", "powercut"] else "boot-one"
        return territory, directory, transaction, operation, boot

    def test_each_fault_requires_its_exact_rollback_operation_and_boot_semantics(self):
        for scenario in journey.FAULT_SCENARIOS:
            with self.subTest(scenario=scenario):
                territory, directory, _, operation, boot = self.finish(scenario)
                self.evidence(territory, directory, scenario, "rolled-back", boot)
                operation["status"] = "succeeded"
                self.write(territory / "run/operations/operation-one.json", operation)
                with self.assertRaisesRegex(AssertionError, "recovery outcome"):
                    self.evidence(territory, directory, scenario, "rolled-back", boot)

    def test_timeout_rejects_early_or_changed_deadlines(self):
        territory, directory, transaction, _, boot = self.finish("timeout")
        transaction["updated_at"] = "2026-11-17T12:09:59Z"
        self.write(territory / "transactions/restore-one.json", transaction)
        with self.assertRaisesRegex(AssertionError, "before the real deadline"):
            self.evidence(territory, directory, "timeout", "rolled-back", boot)
        transaction["restore_confirmation"]["confirmation_deadline"] = "2026-11-17T12:00:01Z"
        self.write(territory / "transactions/restore-one.json", transaction)
        with self.assertRaisesRegex(AssertionError, "deadline changed"):
            self.evidence(territory, directory, "timeout", "rolled-back", boot)

    def test_pending_restore_rejects_a_premature_state_commit(self):
        territory, directory, original, _, _ = self.fixture("explicit")
        self.write(territory / "state/install-state.json", {**original, "last_transaction_id": "restore-one"})
        with self.assertRaisesRegex(AssertionError, "changed committed"):
            self.evidence(territory, directory, "explicit", "pending", "boot-one")


class RuntimeStartupTests(unittest.TestCase):
    def test_runtime_snapshot_allows_equivalent_links_but_rejects_foreign_targets_and_persistent_members(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            etc, run = root / "etc", root / "run"
            etc.mkdir()
            wants = run / "multi-user.target.wants"
            wants.mkdir(parents=True)
            definition = root / "boot-definition"
            definition.write_text("managed unit")
            (etc / "landscape-router.service").symlink_to(definition)
            (run / "landscape-router.service").symlink_to(etc / "landscape-router.service")
            member = wants / "landscape-router.service"
            member.symlink_to("../landscape-router.service")
            with contextlib.redirect_stdout(io.StringIO()):
                journey.runtime_link_evidence(root, "runtime-save", etc, run)
                journey.runtime_link_evidence(root, "runtime-check", etc, run)
            member.unlink()
            member.symlink_to(definition)
            with contextlib.redirect_stdout(io.StringIO()):
                journey.runtime_link_evidence(root, "runtime-check", etc, run)
            foreign = root / "foreign-definition"
            foreign.write_text("foreign unit")
            member.unlink()
            member.symlink_to(foreign)
            with self.assertRaisesRegex(AssertionError, "foreign definition"):
                journey.runtime_link_evidence(root, "runtime-check", etc, run)
            member.unlink()
            member.symlink_to(definition)
            persistent = etc / "multi-user.target.wants"
            persistent.mkdir()
            (persistent / "landscape-router.service").symlink_to("../landscape-router.service")
            with self.assertRaisesRegex(AssertionError, "only runtime"):
                journey.runtime_link_evidence(root, "runtime-check", etc, run)


if __name__ == "__main__":
    unittest.main()
