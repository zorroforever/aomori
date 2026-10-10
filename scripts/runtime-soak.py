#!/usr/bin/env python3
"""Disposable Linux node read/write soak. Reports evidence, not capacity claims."""
import argparse
from concurrent.futures import ThreadPoolExecutor
import json
import hashlib
import os
from pathlib import Path
import socket
import subprocess
import tempfile
import time
import urllib.request


def replay_events(rpc):
    """Hash every event in order without retaining the full history in memory."""
    cursor = count = 0
    digest = hashlib.sha256()
    while True:
        page = rpc('aomori_get_events', {'since': cursor, 'limit': 500})
        for event in page['events']:
            if event['id'] != cursor + 1:
                raise RuntimeError('Event history gap or duplicate')
            cursor = event['id']
            count += 1
            digest.update(json.dumps(event, sort_keys=True, separators=(',', ':')).encode())
            digest.update(b'\n')
        if cursor >= page['latest']:
            if cursor != page['latest']:
                raise RuntimeError('Event exceeds advertised latest cursor')
            return cursor, count, digest.hexdigest()
        if not page['events']:
            raise RuntimeError('History gap')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--seconds', type=int, default=10)
    parser.add_argument('--readers', type=int, default=8)
    parser.add_argument('--restart-mode', choices=['sigterm', 'sigkill'], default='sigterm', help='How to stop after acknowledged writes, before recovery verification')
    parser.add_argument('--interval', type=float, default=0.5, help='Seconds between write batches')
    parser.add_argument('--max-rss-mib', type=int, help='Fail when sampled node RSS exceeds this positive budget')
    parser.add_argument('--max-snapshot-mib', type=int, help='Fail when sampled primary snapshot exceeds this positive budget')
    parser.add_argument('--report', type=Path, required=True, help='New JSON report path; never overwrite')
    parser.add_argument('--binary', type=Path, default=Path(__file__).resolve().parents[1] / 'target/debug/aomori')
    args = parser.parse_args()
    if not 1 <= args.seconds <= 86400 or not 1 <= args.readers <= 32 or not 0.1 <= args.interval <= 60:
        parser.error('seconds: 1..86400, readers: 1..32, interval: 0.1..60')
    if any(value is not None and value <= 0 for value in (args.max_rss_mib, args.max_snapshot_mib)):
        parser.error('resource budgets must be positive integers')
    binary = args.binary.resolve()
    if not binary.is_file():
        parser.error('Build the node first')
    # Reserve the report before starting; a failed run cannot masquerade as a pass.
    with args.report.open('x') as output:
        json.dump({'status': 'running'}, output)
    report = {'status': 'failed', 'settings': {'seconds': args.seconds, 'readers': args.readers, 'restart_mode': args.restart_mode, 'interval': args.interval, 'max_rss_mib': args.max_rss_mib, 'max_snapshot_mib': args.max_snapshot_mib}, 'samples': []}
    process = None
    try:
        with tempfile.TemporaryDirectory(prefix='aomori-soak-') as directory:
            data = Path(directory) / 'data'
            with socket.socket() as reservation:
                reservation.bind(('127.0.0.1', 0))
                port = reservation.getsockname()[1]
            endpoint = f'http://127.0.0.1:{port}'
            env = {k: v for k, v in os.environ.items() if not k.startswith('AOMORI_')}
            env['AOMORI_RPC_RATE_LIMIT'] = '1000'
            logs = open(Path(directory) / 'node.log', 'wb')

            def stop():
                nonlocal process
                if process is not None and process.poll() is None:
                    process.terminate()
                    try:
                        process.wait(timeout=10)
                    except subprocess.TimeoutExpired:
                        process.kill()
                        process.wait()
                        raise RuntimeError('Node did not stop gracefully')

            def start():
                nonlocal process
                process = subprocess.Popen([str(binary), '--listen', f'127.0.0.1:{port}', '--data-dir', str(data), '--demo', '--allow-unsigned-commands'], env=env, stdout=logs, stderr=logs)
                deadline = time.monotonic() + 10
                while time.monotonic() < deadline:
                    if process.poll() is not None:
                        raise RuntimeError('Node exited before readiness')
                    try:
                        with urllib.request.urlopen(endpoint + '/ready', timeout=1) as response:
                            if response.status == 200:
                                return
                    except OSError:
                        pass
                    time.sleep(0.1)
                raise RuntimeError('Node readiness timeout')

            def rpc(method, params=None):
                request = urllib.request.Request(endpoint + '/rpc', data=json.dumps({'jsonrpc': '2.0', 'id': 1, 'method': method, 'params': params or {}}).encode(), headers={'content-type': 'application/json'})
                with urllib.request.urlopen(request, timeout=5) as response:
                    body = json.load(response)
                if 'error' in body:
                    raise RuntimeError(f'{method}: {body["error"]}')
                return body['result']

            def sample(elapsed):
                status = Path(f'/proc/{process.pid}/status').read_text()
                rss = next(int(line.split()[1]) for line in status.splitlines() if line.startswith('VmRSS:'))
                # utime/stime start at positions 14/15; comm can contain spaces.
                fields = Path(f'/proc/{process.pid}/stat').read_text().rsplit(')', 1)[1].split()
                cpu = (int(fields[11]) + int(fields[12])) / os.sysconf('SC_CLK_TCK')
                snapshot_bytes = (data / 'state.json').stat().st_size
                report['samples'].append({'elapsed_seconds': round(elapsed, 3), 'rss_kib': rss, 'cpu_seconds': cpu, 'snapshot_bytes': snapshot_bytes})
                if args.max_rss_mib is not None and rss > args.max_rss_mib * 1024:
                    raise RuntimeError(f'RSS budget exceeded: {rss} KiB > {args.max_rss_mib} MiB')
                if args.max_snapshot_mib is not None and snapshot_bytes > args.max_snapshot_mib * 1024 * 1024:
                    raise RuntimeError(f'Snapshot budget exceeded: {snapshot_bytes} bytes > {args.max_snapshot_mib} MiB')

            try:
                start()
                initial = rpc('aomori_get_info')
                writes = reads = 0
                # First receipt plus logarithmically spaced checkpoints keeps
                # retained receipts bounded even during day-long runs.
                checkpoints = []
                max_batch_ms = 0
                started = time.monotonic()
                sample(0)
                next_sample = 5
                with ThreadPoolExecutor(max_workers=args.readers) as pool:
                    while time.monotonic() - started < args.seconds:
                        before = rpc('aomori_get_info')
                        batch_started = time.monotonic()
                        futures = [pool.submit(rpc, 'aomori_get_info') for _ in range(args.readers)]
                        receipt = rpc('aomori_submit_transaction', {'from': 'admin', 'nonce': writes, 'entity_id': 4, 'action': 'talk', 'args': {'npc_id': 6}, 'signature': None})
                        if not receipt['ok']:
                            raise RuntimeError('Transaction failed')
                        writes += 1
                        if writes & (writes - 1) == 0:
                            checkpoints.append(receipt)
                        after = rpc('aomori_get_info')
                        if after['head'] != before['head'] + 1 or after['state_root'] != receipt['state_root']:
                            raise RuntimeError('Write did not advance exactly one state')
                        for future in futures:
                            result = future.result()
                            if (result['head'], result['state_root']) not in [(before['head'], before['state_root']), (after['head'], after['state_root'])]:
                                raise RuntimeError('Read observed inconsistent state')
                            reads += 1
                        max_batch_ms = max(max_batch_ms, (time.monotonic() - batch_started) * 1000)
                        report.update(writes=writes, concurrent_reads=reads, max_batch_ms=round(max_batch_ms, 3))
                        elapsed = time.monotonic() - started
                        if elapsed >= next_sample:
                            sample(elapsed)
                            next_sample = elapsed + 5
                        time.sleep(args.interval)
                sample(time.monotonic() - started)
                final = rpc('aomori_get_info')
                account = rpc('aomori_get_account', {'name': 'admin'})
                if account['nonce'] != writes or final['head'] != initial['head'] + writes:
                    raise RuntimeError('Final nonce/head mismatch')
                # Replay full paginated history; do not confuse a 500-event page
                # with the entire event log in hour/day runs.
                cursor, count, history_digest = replay_events(rpc)
                if checkpoints[-1]['tx_id'] != receipt['tx_id']:
                    checkpoints.append(receipt)
                report.update(event_history_sha256=history_digest, receipts_checked=len(checkpoints))
                if count != final['events']:
                    raise RuntimeError('Event replay count mismatch')
                if args.restart_mode == 'sigkill':
                    if process.poll() is not None:
                        raise RuntimeError('Node exited before planned SIGKILL')
                    process.kill()
                    exit_code = process.wait(timeout=10)
                    if exit_code != -9:
                        raise RuntimeError(f'Expected SIGKILL exit -9, got {exit_code}')
                else:
                    stop()
                    exit_code = process.returncode
                    if exit_code != 0:
                        raise RuntimeError(f'Graceful restart exit was {exit_code}')
                report['restart_exit_code'] = exit_code
                start()
                if replay_events(rpc) != (cursor, count, history_digest):
                    raise RuntimeError('Restart changed event history')
                for checkpoint in checkpoints:
                    if rpc('aomori_get_receipt', {'tx_id': checkpoint['tx_id']}) != checkpoint:
                        raise RuntimeError('Restart changed checkpoint receipt')
                if rpc('aomori_get_info') != final or rpc('aomori_get_account', {'name': 'admin'}) != account:
                    raise RuntimeError('Restart changed acknowledged state')
                report.update(status='passed', writes=writes, concurrent_reads=reads, events_replayed=count, latest_event=cursor, elapsed_seconds=round(time.monotonic() - started, 3), max_batch_ms=round(max_batch_ms, 3), final_head=final['head'], final_state_root=final['state_root'], restart_verified=True)
            finally:
                stop()
                logs.close()
    except (Exception, KeyboardInterrupt) as error:
        report['error'] = str(error) or type(error).__name__
        raise
    finally:
        samples = report['samples']
        if samples:
            report['resource_summary'] = {
                'sample_count': len(samples),
                'peak_rss_kib': max(item['rss_kib'] for item in samples),
                'peak_snapshot_bytes': max(item['snapshot_bytes'] for item in samples),
                'rss_growth_kib': samples[-1]['rss_kib'] - samples[0]['rss_kib'],
                'snapshot_growth_bytes': samples[-1]['snapshot_bytes'] - samples[0]['snapshot_bytes'],
            }
        args.report.write_text(json.dumps(report, indent=2) + '\n')
    print(f'Soak passed: {report["writes"]} writes, {report["concurrent_reads"]} concurrent reads; report: {args.report}')


if __name__ == '__main__':
    main()
