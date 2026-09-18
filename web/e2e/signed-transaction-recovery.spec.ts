import { expect, test, type Page } from '@playwright/test';

const rpcUrl = 'http://127.0.0.1:18093/rpc';

async function createSignedIdentity(page: Page, account: string) {
  await page.goto('/');
  await page.getByRole('button', { name: '连接节点' }).click();
  await expect(page.locator('#statusText')).toHaveText('节点在线');
  await page.locator('#accountInput').fill(account);
  await page.locator('#adminTokenInput').fill('e2e-admin-token');
  page.once('dialog', dialog => dialog.accept('local-password'));
  await page.getByRole('button', { name: '创建签名身份' }).click();
  await expect(page.locator('#writeMode')).toContainText(`签名交易 · ${account}`);
  await expect(page.locator('#roomEntities')).toContainText('Mira');
}

test('recovers controls after a signed transaction network failure', async ({ page }) => {
  const account = 'network-recovery-player';
  await createSignedIdentity(page, account);

  let failedSubmissions = 0;
  await page.route(rpcUrl, async route => {
    const request = route.request().postDataJSON();
    if (request.method === 'aomori_submit_transaction') {
      failedSubmissions++;
      await route.abort('failed');
    } else await route.continue();
  });

  await page.locator('#roomEntities').getByRole('button', { name: /Mira/ }).click();
  await expect(page.locator('#log')).toContainText('无法连接节点，请检查节点地址或网络连接');
  expect(failedSubmissions).toBe(1);
  await expect(page.locator('#commandInput')).toBeEnabled();
  await expect(page.getByRole('button', { name: '查看' })).toBeEnabled();
  await expect(page.locator('#roomEntities').getByRole('button', { name: /Mira/ })).toBeEnabled();
  await expect(page.locator('#writeMode')).toContainText(`签名交易 · ${account}`);

  await page.unroute(rpcUrl);
  await page.locator('#roomEntities').getByRole('button', { name: /Mira/ }).click();
  await expect(page.locator('#receipt')).toContainText('SUCCESS');
  const response = await page.request.post(rpcUrl, { data: { jsonrpc: '2.0', id: 1, method: 'aomori_get_account', params: { name: account } } });
  expect((await response.json()).result.nonce).toBe(1);
});

test('keeps the identity locked when the node public key does not match', async ({ page }) => {
  const account = 'mismatched-key-player';
  await createSignedIdentity(page, account);
  await page.getByRole('button', { name: '锁定当前会话' }).click();
  await expect(page.locator('#writeMode')).toContainText('身份已锁定');

  await page.route(rpcUrl, async route => {
    const request = route.request().postDataJSON();
    if (request.method === 'aomori_get_account' && request.params.name === account) {
      const response = await route.fetch();
      const body = await response.json();
      body.result.public_key = '00'.repeat(32);
      await route.fulfill({ response, json: body });
    } else await route.continue();
  });

  page.once('dialog', dialog => dialog.accept('local-password'));
  await page.getByRole('button', { name: '解锁本地身份' }).click();
  await expect(page.locator('#log')).toContainText('节点账户与本地身份公钥不匹配');
  await expect(page.locator('#writeMode')).toContainText('身份已锁定');
  await expect(page.locator('#commandInput')).toBeEnabled();

  await page.locator('#roomEntities').getByRole('button', { name: /Mira/ }).click();
  await expect(page.locator('#log')).toContainText('签名身份已锁定，请先解锁本地身份');
});

test('refreshes the nonce and re-signs once after a nonce conflict', async ({ page }) => {
  const account = 'nonce-retry-player';
  await createSignedIdentity(page, account);

  const transactions: Array<{ nonce: number; signature: string }> = [];
  let conflictReturned = false;
  await page.route(rpcUrl, async route => {
    const request = route.request().postDataJSON();
    if (request.method === 'aomori_get_account' && conflictReturned && transactions.length === 1) {
      const response = await route.fetch();
      const body = await response.json();
      body.result.nonce = 1;
      await route.fulfill({ response, json: body });
      return;
    }
    if (request.method === 'aomori_submit_transaction') {
      transactions.push({ nonce: request.params.nonce, signature: request.params.signature });
      if (transactions.length === 1) {
        conflictReturned = true;
        await route.fulfill({ status: 200, json: { jsonrpc: '2.0', id: request.id, error: { code: -32001, message: 'invalid nonce: expected 1, got 0' } } });
      } else {
        await route.fulfill({ status: 200, json: { jsonrpc: '2.0', id: request.id, result: { ok: true, tx_id: 'nonce-retry', state_root: 'test-root', messages: ['nonce retry accepted'] } } });
      }
      return;
    }
    await route.continue();
  });

  await page.locator('#roomEntities').getByRole('button', { name: /Mira/ }).click();
  await expect(page.locator('#receipt')).toContainText('nonce-retry');
  await expect(page.locator('#log')).toContainText('nonce retry accepted');
  expect(transactions).toHaveLength(2);
  expect(transactions.map(transaction => transaction.nonce)).toEqual([0, 1]);
  expect(transactions[0].signature).not.toBe(transactions[1].signature);
  await expect(page.locator('#commandInput')).toBeEnabled();
});
