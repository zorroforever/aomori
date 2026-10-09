import { expect, test } from '@playwright/test';
import { spawn, type ChildProcess } from 'node:child_process';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { resolve, join } from 'node:path';
import { once } from 'node:events';
import { createServer } from 'node:net';

test('recovers a real node restart without losing the unlocked signing identity', async ({ page }) => {
  const reservation = createServer();
  await new Promise<void>(done => reservation.listen(0, '127.0.0.1', done));
  const address = reservation.address();
  if (!address || typeof address === 'string') throw new Error('Missing port');
  const port = address.port;
  await new Promise<void>(done => reservation.close(() => done()));
  const endpoint = `http://127.0.0.1:${port}`;
  const directory = await mkdtemp(join(tmpdir(), 'aomori-lifecycle-'));
  let node: ChildProcess | undefined;
  let logs = '';
  const start = async () => {
    node = spawn(resolve('../target/debug/aomori'), ['--listen', `127.0.0.1:${port}`, '--data-dir', directory, '--demo'], {
      env: { ...process.env, AOMORI_ADMIN_TOKEN: 'lifecycle-test-token', AOMORI_CORS_ORIGINS: 'http://127.0.0.1:15173' },
      stdio: ['ignore', 'ignore', 'pipe'],
    });
    node.stderr!.on('data', chunk => { logs = (logs + chunk.toString()).slice(-16000); });
    await expect.poll(async () => {
      if (node!.exitCode !== null) throw new Error(logs);
      try { return (await fetch(`${endpoint}/ready`)).ok; } catch { return false; }
    }, { timeout: 10000 }).toBe(true);
  };
  const stop = async () => {
    if (!node || node.exitCode !== null) return;
    const exited = once(node, 'exit');
    node.kill('SIGKILL');
    await exited;
  };
  const rpc = async (method: string, params = {}) => {
    const response = await fetch(`${endpoint}/rpc`, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ jsonrpc: '2.0', id: 1, method, params }) });
    const body = await response.json();
    if (body.error) throw new Error(body.error.message);
    return body.result;
  };
  try {
    await start();
    await page.goto('/');
    await page.locator('#rpcInput').fill(endpoint);
    await page.getByRole('button', { name: '连接节点' }).click();
    await expect(page.locator('#connectBtn')).toBeEnabled();
    await page.locator('#accountInput').fill('lifecycle-player');
    await page.locator('#adminTokenInput').fill('lifecycle-test-token');
    page.once('dialog', dialog => dialog.accept('lifecycle-password'));
    await page.getByRole('button', { name: '创建签名身份' }).click();
    await expect(page.getByRole('button', { name: '创建签名身份' })).toBeEnabled();
    await page.locator('#roomEntities').getByRole('button', { name: /Mira/ }).click();
    await expect(page.locator('#receipt')).toContainText('SUCCESS');
    await expect(page.locator('#commandInput')).toBeEnabled();
    const info = await rpc('aomori_get_info');
    const events = await rpc('aomori_get_events', { since: 0, limit: 500 });
    await expect.poll(() => page.evaluate(url => localStorage.getItem(`aomori:event-cursor:${url}`), endpoint)).toBe(String(events.latest));
    const rows = await page.locator('#eventList .event').count();
    await stop();
    await expect(page.locator('#statusText')).toHaveText('事件通道重连中');
    await start();
    expect((await rpc('aomori_get_info')).state_root).toBe(info.state_root);
    await expect(page.locator('#statusText')).toHaveText('节点在线', { timeout: 10000 });
    await expect(page.locator('#writeMode')).toHaveText('签名交易 · lifecycle-player');
    await page.locator('#roomEntities').getByRole('button', { name: /Mira/ }).click();
    await expect(page.locator('#commandInput')).toBeEnabled();
    await expect.poll(async () => (await rpc('aomori_get_account', { name: 'lifecycle-player' })).nonce).toBe(2);
    const recovered = await rpc('aomori_get_events', { since: events.latest, limit: 500 });
    await expect(page.locator('#eventList .event')).toHaveCount(rows + recovered.events.length);
    await expect.poll(() => page.evaluate(url => localStorage.getItem(`aomori:event-cursor:${url}`), endpoint)).toBe(String(recovered.latest));
    await expect(page.locator('#log')).not.toContainText('事件补偿失败');
  } finally {
    await stop();
    await rm(directory, { recursive: true, force: true });
  }
});

// Baseline by default; opt into longer soaks without changing the regular gate.
test('sustains concurrent RPC reads and websocket connections without state mutation', async ({ page }) => {
  const seconds = Number(process.env.AOMORI_SOAK_SECONDS || 5);
  const connections = Number(process.env.AOMORI_SOAK_CONNECTIONS || 32);
  if (!Number.isInteger(seconds) || seconds < 1 || seconds > 86400 || !Number.isInteger(connections) || connections < 1 || connections > 1000) throw new Error('Invalid soak settings');
  test.setTimeout(seconds * 1000 + 30000);
  await page.goto('/');
  const result = await page.evaluate(async ({ seconds, connections }) => {
    const endpoint = 'http://127.0.0.1:18093';
    const rpc = async (method: string) => {
      const response = await fetch(`${endpoint}/rpc`, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ jsonrpc: '2.0', id: 1, method, params: {} }), signal: AbortSignal.timeout(5000) });
      const body = await response.json();
      if (response.status === 429) return { limited: true };
      if (!response.ok || body.error) throw new Error(JSON.stringify(body));
      return body.result;
    };
    const sockets: WebSocket[] = [];
    let reads = 0;
    let limited = 0;
    const before = await rpc('aomori_get_info');
    try {
      // Avoid Chromium's per-host handshake throttling; the load phase still
      // holds all connections concurrently.
      for (let index = 0; index < connections; index++) await new Promise<void>((resolve, reject) => {
        const socket = new WebSocket(`${endpoint.replace('http', 'ws')}/events`);
        sockets.push(socket);
        const timer = setTimeout(() => reject(new Error('Websocket open timeout')), 5000);
        socket.onopen = () => { clearTimeout(timer); resolve(); };
        socket.onerror = () => { clearTimeout(timer); reject(new Error('Websocket failed')); };
      });
      const until = Date.now() + seconds * 1000;
      while (Date.now() < until) {
        const responses = await Promise.all(Array.from({ length: 16 }, () => rpc('aomori_get_info')));
        for (const response of responses) { if (response.limited) limited++; else { reads++; if (response.state_root !== before.state_root) throw new Error('Read changed world state'); } }
        if (sockets.some(socket => socket.readyState !== WebSocket.OPEN)) throw new Error('Unexpected socket close');
        await new Promise(resolve => setTimeout(resolve, 250));
      }
      return { reads, limited };
    } finally { sockets.forEach(socket => socket.close()); }
  }, { seconds, connections });
  expect(result.reads).toBeGreaterThan(0);
  await expect.poll(async () => {
    const response = await fetch('http://127.0.0.1:18093/metrics');
    return (await response.json()).websocket.active;
  }).toBe(0);
});
