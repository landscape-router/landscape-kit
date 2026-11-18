"""Pure parser and temporary-file migration checks; no processes or sockets."""

import contextlib
import importlib.util
import io
import os
from pathlib import Path
import stat
import tempfile
import unittest
from unittest.mock import patch


SPEC = importlib.util.spec_from_file_location(
    'upgrade_guest', Path(__file__).with_name('upgrade-guest.py'))
upgrade = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(upgrade)
UNIX_HEADER = 'Num       RefCount Protocol Flags    Type St Inode Path\n'


def unix_entry(path=None):
    return '0000000000000000: 00000002 00000000 00010000 0001 01 1234' + (
        ' ' + str(path) if path is not None else '') + '\n'


class MigrationSocketTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.directory = Path(temporary.name)
        self.root = self.directory / 'landscape'
        self.socket = self.root / 'data/unix_link/register.sock'
        self.socket.parent.mkdir(parents=True)
        self.addCleanup(patch.stopall)
        patch.object(upgrade, 'run', side_effect=AssertionError('no real processes allowed')).start()
        patch.object(upgrade, 'command', side_effect=AssertionError('no real processes allowed')).start()

    @staticmethod
    @contextlib.contextmanager
    def socket_metadata(*paths):
        # Regular temporary files model socket inodes; never bind a Unix socket.
        original = Path.lstat

        def lstat(path, *args, **kwargs):
            result = original(path, *args, **kwargs)
            if path in paths:
                values = list(result)
                values[0] = stat.S_IFSOCK | stat.S_IMODE(result.st_mode)
                return os.stat_result(values)
            return result

        with patch.object(Path, 'lstat', lstat):
            yield

    def remove(self, table=UNIX_HEADER):
        with contextlib.redirect_stdout(io.StringIO()):
            upgrade.remove_stale_registration_socket(self.root, table)

    def test_stopped_check_requires_both_units_inactive_with_zero_pid(self):
        with patch.object(upgrade, 'run', return_value='MainPID=0\nActiveState=inactive') as query:
            upgrade.assert_stopped_services()
        self.assertEqual([call.args[2] for call in query.call_args_list],
                         [upgrade.UNIT, 'lkit.service'])
        for failing_unit in (upgrade.UNIT, 'lkit.service'):
            for state, pid in [('active', '12'), ('inactive', '12'), ('active', '0'), ('failed', '0')]:
                with self.subTest(unit=failing_unit, state=state, pid=pid):
                    def facts(*args):
                        return (f'MainPID={pid}\nActiveState={state}' if args[2] == failing_unit
                                else 'MainPID=0\nActiveState=inactive')
                    with patch.object(upgrade, 'run', side_effect=facts):
                        with self.assertRaisesRegex(AssertionError, failing_unit):
                            upgrade.assert_stopped_services()

    def test_proc_parser_preserves_path_spaces_and_ignores_unnamed_sockets(self):
        table = UNIX_HEADER + unix_entry() + unix_entry('/tmp/name with spaces/register.sock')
        self.assertEqual(upgrade.unix_socket_paths(table), {'/tmp/name with spaces/register.sock'})
        for table in ['', 'unexpected header\n', UNIX_HEADER + 'bad row\n']:
            with self.assertRaises(AssertionError):
                upgrade.unix_socket_paths(table)

    def test_only_the_known_unheld_runtime_socket_is_removed(self):
        self.socket.write_bytes(b'model socket')
        persistent = self.root / 'data/persistent.db'
        persistent.write_bytes(b'keep this database')
        link = self.root / 'data/user-link'
        link.symlink_to('persistent.db')
        with self.socket_metadata(self.socket):
            self.remove(UNIX_HEADER + unix_entry('/unrelated/socket'))
        self.assertFalse(self.socket.exists())
        self.assertEqual(persistent.read_bytes(), b'keep this database')
        self.assertEqual(os.readlink(link), 'persistent.db')

    def test_held_socket_or_invalid_proc_table_preserves_the_known_entry(self):
        self.socket.write_bytes(b'model socket')
        for table in [UNIX_HEADER + unix_entry(self.socket), 'unreadable table\n']:
            with self.socket_metadata(self.socket):
                with self.assertRaises(AssertionError):
                    self.remove(table)
            self.assertEqual(self.socket.read_bytes(), b'model socket')

    def test_unknown_socket_aborts_before_removing_the_known_socket(self):
        unknown = self.directory / 'user.sock'
        self.socket.write_bytes(b'known model socket')
        unknown.write_bytes(b'unknown model socket')
        with self.socket_metadata(self.socket, unknown):
            with self.assertRaisesRegex(AssertionError, str(unknown)):
                self.remove()
        self.assertEqual(self.socket.read_bytes(), b'known model socket')
        self.assertEqual(unknown.read_bytes(), b'unknown model socket')

    def test_a_regular_file_or_symlink_at_the_socket_name_is_never_removed(self):
        self.socket.write_bytes(b'user file')
        with self.assertRaisesRegex(AssertionError, 'not a socket'):
            self.remove()
        self.assertEqual(self.socket.read_bytes(), b'user file')
        self.socket.unlink()
        target = self.directory / 'user-target'
        target.write_bytes(b'keep target')
        self.socket.symlink_to(target)
        with self.assertRaisesRegex(AssertionError, 'not a socket'):
            self.remove()
        self.assertTrue(self.socket.is_symlink())
        self.assertEqual(target.read_bytes(), b'keep target')

    def test_symlinked_parent_is_rejected_without_touching_its_target(self):
        self.socket.parent.rmdir()
        outside = self.directory / 'outside'
        outside.mkdir()
        target = outside / 'register.sock'
        target.write_bytes(b'keep target')
        self.socket.parent.symlink_to(outside, target_is_directory=True)
        with self.assertRaisesRegex(AssertionError, 'not a real directory'):
            self.remove()
        self.assertEqual(target.read_bytes(), b'keep target')

    def test_missing_known_socket_is_allowed_and_other_sockets_remain_rejected(self):
        self.remove()
        unknown = self.root / 'data/unknown.sock'
        unknown.write_bytes(b'model socket')
        with self.socket_metadata(unknown):
            with self.assertRaisesRegex(AssertionError, str(unknown)):
                upgrade.tree_identity(self.directory)
        self.assertTrue(unknown.exists())


if __name__ == '__main__':
    unittest.main()
