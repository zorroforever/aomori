import { expect, test } from '@playwright/test';

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
