#!/usr/bin/env python3
"""Integration tests of soak reporting and resource-budget failure semantics."""
import json
import importlib.util
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / 'scripts/runtime-soak.py'
BINARY = ROOT / 'target/debug/aomori'


sys.dont_write_bytecode = True
spec = importlib.util.spec_from_file_location('runtime_soak', SCRIPT)
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


class EventReplayTests(unittest.TestCase):
    def replay(self, pages):
        iterator = iter(pages)
        return module.replay_events(lambda *_: next(iterator))

    def test_digest_is_independent_of_pagination_and_object_key_order(self):
        a = {'id': 1, 'data': {'a': 1, 'b': 2}}
        b = {'id': 2, 'data': {}}
        whole = self.replay([{'events': [a, b], 'latest': 2}])
        split = self.replay([{'events': [{'data': {'b': 2, 'a': 1}, 'id': 1}], 'latest': 2}, {'events': [b], 'latest': 2}])
        self.assertEqual(whole, split)
        changed = self.replay([{'events': [{'id': 1, 'data': {'a': 3}}, b], 'latest': 2}])
        self.assertNotEqual(whole[2], changed[2])

    def test_gaps_duplicates_and_invalid_latest_fail(self):
        for page in [
            {'events': [{'id': 2}], 'latest': 2},
            {'events': [{'id': 1}, {'id': 1}], 'latest': 1},
            {'events': [], 'latest': 1},
            {'events': [{'id': 1}], 'latest': 0},
        ]:
            with self.subTest(page=page), self.assertRaises(RuntimeError):
                self.replay([page])


@unittest.skipUnless(BINARY.is_file(), 'Build the node before running integration tests')
class SoakTests(unittest.TestCase):
    def run_soak(self, path, *options):
        return subprocess.run([sys.executable, str(SCRIPT), '--seconds', '1', '--readers', '2', '--report', str(path), *options], capture_output=True, text=True, timeout=30)

    def test_success_report_includes_restart_and_resource_summary(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'report.json'
            result = self.run_soak(path, '--max-rss-mib', '512', '--max-snapshot-mib', '16')
            self.assertEqual(result.returncode, 0, result.stderr)
            report = json.loads(path.read_text())
            self.assertEqual(report['status'], 'passed')
            self.assertTrue(report['restart_verified'])
            self.assertEqual(len(report['event_history_sha256']), 64)
            self.assertGreater(report['receipts_checked'], 0)
            self.assertGreater(report['writes'], 0)
            self.assertEqual(report['concurrent_reads'], report['writes'] * 2)
            self.assertGreater(report['resource_summary']['peak_rss_kib'], 0)

    def test_rss_budget_failure_preserves_sample_and_failed_status(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'report.json'
            result = self.run_soak(path, '--max-rss-mib', '1')
            self.assertNotEqual(result.returncode, 0)
            report = json.loads(path.read_text())
            self.assertEqual(report['status'], 'failed')
            self.assertIn('RSS budget exceeded', report['error'])
            self.assertGreater(report['resource_summary']['peak_rss_kib'], 1024)

    def test_existing_report_is_never_overwritten(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'report.json'
            path.write_text('original evidence')
            self.assertNotEqual(self.run_soak(path).returncode, 0)
            self.assertEqual(path.read_text(), 'original evidence')

    def test_invalid_budget_does_not_create_report(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'report.json'
            self.assertNotEqual(self.run_soak(path, '--max-snapshot-mib', '0').returncode, 0)
            self.assertFalse(path.exists())


if __name__ == '__main__':
    unittest.main()
