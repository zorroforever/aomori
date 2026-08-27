import { expect, test } from '@playwright/test';

test('completes the lost key quest through the UI', async ({ page }) => {
  let limitedRead = true;
  await page.route('http://127.0.0.1:18093/rpc', async route => {
    const request = route.request().postDataJSON();
    if (limitedRead && request.method === 'aomori_get_info') {
      limitedRead = false;
      await route.fulfill({ status: 429, headers: { 'content-type': 'application/json', 'retry-after': '1' }, json: { jsonrpc: '2.0', id: null, error: { code: -32004, message: 'rate limit exceeded', data: { retry_after_ms: 1 } } } });
    } else await route.continue();
  });
  await page.goto('/');
  await page.getByRole('button', { name: '连接节点' }).click();
  await expect(page.locator('#statusText')).toHaveText('节点在线');
  expect(limitedRead).toBe(false);
  await page.unroute('http://127.0.0.1:18093/rpc');

  let limitedWrites = 0;
  await page.route('http://127.0.0.1:18093/rpc', async route => {
    if (route.request().postDataJSON().method === 'aomori_create_account') {
      limitedWrites++;
      await route.fulfill({ status: 429, headers: { 'content-type': 'application/json', 'retry-after': '1' }, json: { jsonrpc: '2.0', id: null, error: { code: -32004, message: 'rate limit exceeded', data: { retry_after_ms: 25 } } } });
    } else await route.continue();
  });
  await expect(page.locator('#rpcInput')).toHaveValue('http://127.0.0.1:18093');
  await page.locator('#accountInput').fill('browser-player');
  await page.locator('#adminTokenInput').fill('e2e-admin-token');
  await page.getByRole('button', { name: '创建签名身份' }).click();
  await expect(page.locator('#log')).toContainText('请求过于频繁，请在 25 毫秒后重试');
  expect(limitedWrites).toBe(1);
  await page.unroute('http://127.0.0.1:18093/rpc');
  await page.getByRole('button', { name: '创建签名身份' }).click();

  await expect(page.locator('#statusText')).toHaveText('节点在线');
  await expect(page.locator('#writeMode')).toContainText('签名交易');
  await expect(page.locator('#actorInput')).toHaveValue('9');

  page.once('dialog', dialog => dialog.accept('backup-password'));
  const downloadPromise = page.waitForEvent('download');
  await page.getByRole('button', { name: '导出加密备份' }).click();
  const download = await downloadPromise;
  const backupPath = await download.path();
  expect(backupPath).toBeTruthy();
  await page.getByRole('button', { name: '清除此账户私钥' }).click();
  await expect(page.locator('#writeMode')).toHaveText('开发 command');
  page.once('dialog', dialog => dialog.accept('backup-password'));
  await page.locator('#importIdentityFile').setInputFiles(backupPath!);
  await expect(page.locator('#writeMode')).toContainText('签名交易');
  await expect(page.locator('#zoneName')).toHaveText('Village');
  await expect(page.locator('#questList')).toContainText('The Lost Key');
  await expect(page.locator('#questList')).toContainText('available');
  await expect(page.locator('#roomEntities')).toContainText('Mira');

  await page.locator('#roomEntities').getByRole('button', { name: /Mira/ }).evaluate((button: HTMLButtonElement) => { button.click(); button.click(); });
  await expect.poll(async () => {
    const response = await page.request.post('http://127.0.0.1:18093/rpc', { data: { jsonrpc: '2.0', id: 3, method: 'aomori_get_account', params: { name: 'browser-player' } } });
    return (await response.json()).result.nonce;
  }).toBe(2);

  await page.getByRole('button', { name: '接取 The Lost Key' }).click();
  await page.getByRole('button', { name: '接取 Echoes in Stone' }).click();
  await expect(page.locator('#questList')).toContainText('accepted');

  await page.locator('#exits').getByRole('button', { name: /east/ }).click();
  await expect(page.locator('#zoneName')).toHaveText('Forest');
  await expect(page.locator('#roomEntities')).toContainText('brass key');

  await page.locator('#roomEntities').getByRole('button', { name: /brass key/ }).click();
  await expect(page.locator('#inventory')).toContainText('brass key');

  await page.locator('#exits').getByRole('button', { name: /east/ }).click();
  await expect(page.locator('#zoneName')).toHaveText('Ruins');
  await expect(page.locator('#roomEntities')).toContainText('stone tablet');
  await page.locator('#roomEntities').getByRole('button', { name: /stone tablet/ }).click();
  await expect(page.locator('#inventory')).toContainText('stone tablet');

  await page.locator('#exits').getByRole('button', { name: /west/ }).click();
  await expect(page.locator('#zoneName')).toHaveText('Forest');
  await page.locator('#exits').getByRole('button', { name: /west/ }).click();
  await expect(page.locator('#zoneName')).toHaveText('Village');
  await page.locator('#questList').getByText('The Lost Key').locator('..').locator('..').getByRole('button', { name: '完成任务' }).click();
  await expect(page.locator('#questList').getByText('The Open Shrine').locator('..').locator('..')).toContainText('available');
  await page.getByRole('button', { name: '接取 The Open Shrine' }).click();
  await page.locator('#questList').getByText('Echoes in Stone').locator('..').locator('..').getByRole('button', { name: '完成任务' }).click();
  await page.locator('#exits').getByRole('button', { name: /east/ }).click();
  await page.locator('#exits').getByRole('button', { name: /east/ }).click();
  await expect(page.locator('#zoneName')).toHaveText('Ruins');
  await page.locator('#questList').getByText('The Open Shrine').locator('..').locator('..').getByRole('button', { name: '完成任务' }).click();

  await expect(page.locator('#questList')).toContainText('completed');
  await expect(page.locator('#inventory')).not.toContainText('brass key');
  await expect(page.locator('#inventory')).toContainText('stone tablet');
  await expect(page.locator('#balance')).toHaveText('20');
  await expect(page.locator('#receipt')).toContainText('SUCCESS');
  await expect(page.locator('#eventList')).toContainText('quest_completed');

  await page.reload();
  await page.locator('#actorInput').fill('9');
  await page.getByRole('button', { name: '连接节点' }).click();
  await expect(page.locator('#writeMode')).toContainText('签名交易');
  await expect(page.locator('#questList')).toContainText('completed');
  await expect(page.locator('#balance')).toHaveText('20');
});
