import { expect, test } from '@playwright/test';

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
