#!/usr/bin/env python3
"""Loopback-only, bounded-input Alertmanager receiver for disposable smoke tests."""
import json
import sys
from http.server import BaseHTTPRequestHandler, HTTPServer


class Receiver(BaseHTTPRequestHandler):
    def do_POST(self):
        if self.path != '/alerts':
            self.send_error(404)
            return
        try:
            length = int(self.headers.get('Content-Length', '0'))
            if not 0 < length <= 65536:
                raise ValueError('Invalid body length')
            body = json.loads(self.rfile.read(length))
            alerts = body['alerts']
            if not isinstance(alerts, list) or len(alerts) > 100:
                raise ValueError('Invalid alert list')
            rows = []
            for alert in alerts:
                status = alert['status']
                name = alert['labels']['alertname']
                if status not in ('firing', 'resolved') or not isinstance(name, str) or len(name) > 256:
                    raise ValueError('Invalid alert')
                rows.append({'status': status, 'alertname': name})
        except (ValueError, KeyError, TypeError):
            self.send_error(400)
            return
        with open(sys.argv[2], 'a', encoding='utf-8') as output:
            for row in rows:
                output.write(json.dumps(row) + '\n')
        self.send_response(200)
        self.end_headers()
        self.wfile.write(b'ok')

    def log_message(self, *_):
        pass


if __name__ == '__main__':
    server = HTTPServer(('127.0.0.1', int(sys.argv[1])), Receiver)
    server.timeout = 5
    # HTTPServer is single-threaded; bound individual client reads too.
    original_get_request = server.get_request

    def timed_request():
        connection, address = original_get_request()
        connection.settimeout(5)
        return connection, address

    server.get_request = timed_request
    server.serve_forever()
