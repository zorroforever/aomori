import { expect, test } from '@playwright/test';

test('clears the active identity when switching RPC endpoints', async ({ page }) => {
  const account = `rpc-switch-${Date.now()}`;
  await page.goto('/');
  await page.getByRole('button', { name: '连接节点' }).click();
  await expect(page.locator('#statusText')).toHaveText('节点在线');
  await page.locator('#accountInput').fill(account);
  await page.locator('#adminTokenInput').fill('e2e-admin-token');
  page.once('dialog', dialog => dialog.accept('local-password'));
  await page.getByRole('button', { name: '创建签名身份' }).click();
  await expect(page.locator('#writeMode')).toContainText(`签名交易 · ${account}`);
  await expect(page.locator('#roomEntities')).toContainText('Mira');

  await page.locator('#rpcInput').fill('http://127.0.0.1:19999');
  await page.getByRole('button', { name: '连接节点' }).click();
  await expect(page.locator('#statusText')).toHaveText('连接失败');
  await expect(page.locator('#writeMode')).toHaveText('开发 command');
  await expect(page.getByRole('button', { name: '锁定当前会话' })).toBeHidden();
  await expect(page.locator('#roomEntities')).toContainText('执行 look 查看');
  await expect(page.locator('#receipt')).toContainText('暂无交易');
});

test('shows a recoverable connection failure state', async ({ page }) => {
  await page.goto('/');
  await page.locator('#rpcInput').fill('http://127.0.0.1:19999');
  await page.getByRole('button', { name: '连接节点' }).click();

  await expect(page.locator('#statusText')).toHaveText('连接失败');
  await expect(page.locator('#connectBtn')).toBeEnabled();
  await expect(page.locator('#log')).toContainText('无法连接节点，请检查节点地址或网络连接');
});
