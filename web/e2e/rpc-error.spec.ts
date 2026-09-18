import { expect, test } from '@playwright/test';

test('reports an HTTP error returned by the node', async ({ page }) => {
  await page.route('http://127.0.0.1:18093/rpc', async route => {
    await route.fulfill({ status: 500, contentType: 'application/json', body: JSON.stringify({ jsonrpc: '2.0', id: null, error: { code: -32603, message: 'internal server error' } }) });
  });

  await page.goto('/');
  await page.getByRole('button', { name: '连接节点' }).click();

  await expect(page.locator('#statusText')).toHaveText('连接失败');
  await expect(page.locator('#log')).toContainText('internal server error');
});

test('reports an invalid response body from the node', async ({ page }) => {
  await page.route('http://127.0.0.1:18093/rpc', async route => {
    await route.fulfill({ status: 200, contentType: 'text/plain', body: 'not-json' });
  });

  await page.goto('/');
  await page.getByRole('button', { name: '连接节点' }).click();

  await expect(page.locator('#statusText')).toHaveText('连接失败');
  await expect(page.locator('#log')).toContainText('节点返回了无效响应');
});
