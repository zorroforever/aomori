import { expect, test } from '@playwright/test';

test('retries compensation on reconnect after an earlier recovery fails', async ({ page }) => {
  let firstSocket: any;
  let connections = 0;
  let eventReads = 0;
  let releaseFirstRead!: () => void;
  const firstReadRelease = new Promise<void>(resolve => { releaseFirstRead = resolve; });
  await page.route('http://127.0.0.1:18093/rpc', async route => {
    const request = route.request().postDataJSON();
    if (request.method !== 'aomori_get_events') return route.continue();
    eventReads++;
    if (eventReads === 1) {
      await firstReadRelease;
      await route.abort('failed');
      return;
    }
    await route.fulfill({ status: 200, json: { jsonrpc: '2.0', id: request.id, result: {
      events: [{ id: 920001, head: 920001, kind: 'reconnected_recovery', data: { value: 'restored' } }],
      next: 920001, latest: 920001,
    } } });
  });
  await page.routeWebSocket('ws://127.0.0.1:18093/events', ws => {
    connections++;
    ws.connectToServer();
    if (connections === 1) firstSocket = ws;
  });

  await page.goto('/');
  await page.getByRole('button', { name: '连接节点' }).click();
  await expect.poll(() => eventReads).toBe(1);
  await firstSocket.close({ code: 1001, reason: 'test disconnect' });
  await expect.poll(() => connections, { timeout: 10_000 }).toBe(2);
  releaseFirstRead();
  await expect.poll(() => eventReads).toBe(2);
  await expect(page.locator('#eventList')).toContainText('reconnected_recovery');
  await expect(page.locator('#eventList .event-id').filter({ hasText: '#920001' })).toHaveCount(1);
});

test('reconnects the event stream after a dropped socket', async ({ page }) => {
  let connectionCount = 0;
  let firstPageSocket: any;

  await page.routeWebSocket('ws://127.0.0.1:18093/events', ws => {
    connectionCount++;
    ws.connectToServer();
    if (connectionCount === 1) firstPageSocket = ws;
  });

  await page.goto('/');
  await page.getByRole('button', { name: '连接节点' }).click();
  await expect(page.locator('#statusText')).toHaveText('节点在线');
  await expect.poll(() => connectionCount).toBe(1);

  await firstPageSocket.close({ code: 1001, reason: 'test disconnect' });
  await expect(page.locator('#log')).toContainText('实时事件通道已断开，准备重连');
  await expect.poll(() => connectionCount, { timeout: 10_000 }).toBe(2);
  await expect(page.locator('#statusText')).toHaveText('节点在线');
});
