import importlib.util
from pathlib import Path
import unittest
from unittest.mock import patch
import tempfile
import subprocess

spec = importlib.util.spec_from_file_location(
    'selection', Path(__file__).with_name('run-test-selection.py'))
selection = importlib.util.module_from_spec(spec)
spec.loader.exec_module(selection)


class InventoryTests(unittest.TestCase):
    def test_stale_filter_is_visible_and_unselected_tests_are_reported(self):
        report = selection.inventory('a::one: test\nb::two: test\n2 tests, 0 benchmarks\n',
                                     ['a::', 'removed::'])
        self.assertEqual(report['empty_filters'], ['removed::'])
        self.assertEqual(report['unselected'], ['b::two'])
        self.assertEqual(report['selected_count'], 1)

    def test_empty_listing_or_duplicate_filters_cannot_pass(self):
        for listing, filters in [('', ['a']), ('a: test\n', []),
                                 ('a: test\n', ['a', 'a']), ('a: test\na: test\n', ['a'])]:
            with self.assertRaises(ValueError):
                selection.inventory(listing, filters)

    def test_overlapping_filters_do_not_inflate_coverage(self):
        report = selection.inventory('a::one: test\na::two: test\n', ['a::', 'a::one'])
        self.assertEqual(report['selected_count'], 2)

    def test_zero_match_aborts_before_execution_and_keeps_inventory(self):
        with tempfile.TemporaryDirectory() as directory:
            report = Path(directory) / 'inventory.json'
            argv = ['runner', '--filter', 'removed::', '--report', str(report), '--', 'fake-binary']
            with patch('sys.argv', argv), patch.object(selection.subprocess, 'run') as run:
                run.return_value.stdout = 'present::one: test\n'
                with self.assertRaisesRegex(ValueError, 'zero tests'):
                    selection.main()
                self.assertEqual(run.call_count, 1)
                self.assertTrue(report.is_file())

    def test_execution_failure_is_not_retried_or_hidden(self):
        with tempfile.TemporaryDirectory() as directory:
            argv = ['runner', '--filter', 'a::', '--filter', 'b::', '--report',
                    str(Path(directory) / 'inventory.json'), '--', 'cargo', 'test', '--bin', 'lkit']
            listed = subprocess.CompletedProcess([], 0, stdout='a::one: test\nb::two: test\n')
            with patch('sys.argv', argv), patch.object(selection.subprocess, 'run') as run:
                run.side_effect = [listed, subprocess.CalledProcessError(101, ['cargo'])]
                with self.assertRaises(subprocess.CalledProcessError):
                    selection.main()
                self.assertEqual(run.call_count, 2)
                self.assertEqual(run.call_args_list[0].args[0][-2:], ['--', '--list'])


if __name__ == '__main__':
    unittest.main()
