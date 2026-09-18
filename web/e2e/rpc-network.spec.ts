import { expect, test } from '@playwright/test';

test('reports a recoverable RPC network failure', async ({ page }) => {
  await page.route('http://127.0.0.1:18093/rpc', route => route.abort('failed'));

  await page.goto('/');
  await page.getByRole('button', { name: '连接节点' }).click();

  await expect(page.locator('#statusText')).toHaveText('连接失败');
  await expect(page.locator('#log')).toContainText('无法连接节点，请检查节点地址或网络连接');
  await expect(page.locator('#connectBtn')).toBeEnabled();
});
