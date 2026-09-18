import { expect, test } from '@playwright/test';

test('shows a recoverable connection failure state', async ({ page }) => {
  await page.goto('/');
  await page.locator('#rpcInput').fill('http://127.0.0.1:19999');
  await page.getByRole('button', { name: '连接节点' }).click();

  await expect(page.locator('#statusText')).toHaveText('连接失败');
  await expect(page.locator('#connectBtn')).toBeEnabled();
  await expect(page.locator('#log')).toContainText('Failed to fetch');
});
