import { expect, test } from '@playwright/test';
import { createServer } from 'node:http';

test('times out when response headers arrive but the body stalls', async ({ page }) => {
  const server = createServer((request, response) => {
    response.setHeader('Access-Control-Allow-Origin', 'http://127.0.0.1:15173');
    response.setHeader('Access-Control-Allow-Headers', 'content-type');
    if (request.method === 'OPTIONS') { response.writeHead(204); response.end(); return; }
    response.writeHead(200, { 'Content-Type': 'application/json' });
    response.flushHeaders();
    response.write('{"jsonrpc":"2.0","result":');
  });
  await new Promise<void>(resolve => server.listen(0, '127.0.0.1', resolve));
  const address = server.address();
  if (!address || typeof address === 'string') throw new Error('Missing test server address');
  try {
    await page.goto('/');
    await page.locator('#rpcInput').fill(`http://127.0.0.1:${address.port}`);
    await page.getByRole('button', { name: '连接节点' }).click();
    await expect(page.locator('#statusText')).toHaveText('连接失败', { timeout: 8_000 });
    await expect(page.locator('#connectBtn')).toBeEnabled();
    await expect(page.locator('#log')).toContainText('RPC 请求超时，请检查节点连接');
    await expect(page.locator('#log')).not.toContainText('节点返回了无效响应');
  } finally {
    server.closeAllConnections();
    await new Promise<void>((resolve, reject) => server.close(error => error ? reject(error) : resolve()));
  }
});

test('cancels a stalled response body immediately when switching RPC', async ({ page }) => {
  let bodyStarted = false;
  let bodyClosed = false;
  const server = createServer(async (request, response) => {
    response.setHeader('Access-Control-Allow-Origin', 'http://127.0.0.1:15173');
    response.setHeader('Access-Control-Allow-Headers', 'content-type');
    if (request.method === 'OPTIONS') { response.writeHead(204); response.end(); return; }
    try {
      let body = '';
      for await (const chunk of request) body += chunk;
      if (JSON.parse(body).method === 'aomori_get_events') {
        response.writeHead(200, { 'Content-Type': 'application/json' });
        response.write('{"jsonrpc":"2.0","result":');
        bodyStarted = true;
        response.on('close', () => { bodyClosed = true; });
        return;
      }
      const upstream = await fetch('http://127.0.0.1:18093/rpc', {
        method: 'POST', headers: { 'content-type': 'application/json' }, body,
      });
      response.writeHead(upstream.status, { 'Content-Type': 'application/json' });
      response.end(await upstream.text());
    } catch { response.destroy(); }
  });
  // A mocked open socket starts compensation against the real HTTP server.
  await page.routeWebSocket(/ws:\/\/127\.0\.0\.1:\d+\/events/, () => undefined);
  await new Promise<void>(resolve => server.listen(0, '127.0.0.1', resolve));
  const address = server.address();
  if (!address || typeof address === 'string') throw new Error('Missing test server address');
  try {
    await page.goto('/');
    await page.locator('#rpcInput').fill(`http://127.0.0.1:${address.port}`);
    await page.getByRole('button', { name: '连接节点' }).click();
    await expect.poll(() => bodyStarted).toBe(true);
    await expect(page.locator('#connectBtn')).toBeEnabled();
    await page.locator('#rpcInput').fill('http://127.0.0.1:18093');
    await page.getByRole('button', { name: '连接节点' }).click();
    await expect.poll(() => bodyClosed, { timeout: 2_000 }).toBe(true);
    await expect(page.locator('#connectBtn')).toBeEnabled();
    await expect(page.locator('#statusText')).toHaveText('节点在线');
    await expect(page.locator('#log')).not.toContainText('RPC 请求超时');
    await expect(page.locator('#log')).not.toContainText('无法连接节点');
    await expect(page.locator('#log')).not.toContainText('事件补偿失败');
  } finally {
    server.closeAllConnections();
    await new Promise<void>((resolve, reject) => server.close(error => error ? reject(error) : resolve()));
  }
});

test('reports an RPC timeout when the node does not respond', async ({ page }) => {
  await page.route('http://127.0.0.1:18093/rpc', async route => {
    await new Promise(resolve => setTimeout(resolve, 6_000));
    await route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify({ jsonrpc: '2.0', id: null, result: {} }) });
  });

  await page.goto('/');
  await page.getByRole('button', { name: '连接节点' }).click();

  await expect(page.locator('#statusText')).toHaveText('连接失败', { timeout: 8_000 });
  await expect(page.locator('#connectBtn')).toBeEnabled();
  await expect(page.locator('#log')).toContainText('RPC 请求超时，请检查节点连接');
});
