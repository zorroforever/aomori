import { expect, test } from '@playwright/test';

test('reports malformed event stream messages without breaking the connection', async ({ page }) => {
  await page.routeWebSocket('ws://127.0.0.1:18093/events', ws => {
    const server = ws.connectToServer();
    server.onMessage(() => undefined);
    setTimeout(() => ws.send('not-json'), 300);
  });

  await page.goto('/');
  await page.getByRole('button', { name: '连接节点' }).click();
  await expect(page.locator('#statusText')).toHaveText('节点在线');
  await expect(page.locator('#log')).toContainText('事件通道返回了无效 JSON');
  await expect(page.locator('#statusText')).toHaveText('节点在线');
});
