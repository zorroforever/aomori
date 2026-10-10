#!/usr/bin/env python3
"""Integration tests of soak reporting and resource-budget failure semantics."""
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / 'scripts/runtime-soak.py'
BINARY = ROOT / 'target/debug/aomori'


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
