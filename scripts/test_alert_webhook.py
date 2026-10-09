#!/usr/bin/env python3
"""Receiver protocol tests; no Docker or external services required."""
import importlib.util
import json
from pathlib import Path
import sys
import tempfile
import threading
import unittest
from http.client import HTTPConnection
from http.server import HTTPServer

sys.dont_write_bytecode = True
spec = importlib.util.spec_from_file_location('alert_webhook', Path(__file__).with_name('alert-webhook.py'))
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


class ReceiverTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.output = Path(self.directory.name) / 'notifications.jsonl'
        self.argv = sys.argv
        sys.argv = ['receiver', '0', str(self.output)]
        self.server = HTTPServer(('127.0.0.1', 0), module.Receiver)
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()

    def tearDown(self):
        self.server.shutdown()
        self.server.server_close()
        self.thread.join()
        sys.argv = self.argv
        self.directory.cleanup()

    def post(self, body, path='/alerts'):
        connection = HTTPConnection('127.0.0.1', self.server.server_port, timeout=5)
        try:
            connection.request('POST', path, body, {'Content-Type': 'application/json'})
            response = connection.getresponse()
            response.read()
            return response.status
        finally:
            connection.close()

    def test_records_firing_and_resolved_without_sensitive_fields(self):
        for status in ['firing', 'resolved']:
            payload = {'alerts': [{'status': status, 'labels': {'alertname': 'AomoriNodeUnavailable', 'private': 'not-retained'}, 'annotations': {'secret': 'not-retained'}}]}
            self.assertEqual(self.post(json.dumps(payload)), 200)
        rows = [json.loads(line) for line in self.output.read_text().splitlines()]
        self.assertEqual(rows, [{'status': status, 'alertname': 'AomoriNodeUnavailable'} for status in ['firing', 'resolved']])

    def test_rejects_malformed_messages_atomically(self):
        for body in ['not-json', 'null', '{}', '{"alerts":{}}', '{"alerts":[{"status":"unknown","labels":{"alertname":"bad"}}]}', '{"alerts":[{"status":"firing","labels":{"alertname":"good"}},null]}']:
            self.assertEqual(self.post(body), 400)
        self.assertFalse(self.output.exists())

    def test_rejects_oversized_requests(self):
        self.assertEqual(self.post('x' * 65537), 400)
        self.assertFalse(self.output.exists())

    def test_rejects_unknown_paths(self):
        self.assertEqual(self.post('{}', '/other'), 404)
        self.assertFalse(self.output.exists())


if __name__ == '__main__':
    unittest.main()
