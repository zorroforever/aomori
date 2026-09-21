import { expect, test, type Page } from '@playwright/test';

const rpcUrl = 'http://127.0.0.1:18093/rpc';
test.describe.configure({ mode: 'serial' });

async function createSignedIdentity(page: Page, account: string) {
  await page.goto('/');
  await page.getByRole('button', { name: '连接节点' }).click();
  await expect(page.locator('#statusText')).toHaveText('节点在线');
  await page.locator('#accountInput').fill(account);
  await page.locator('#adminTokenInput').fill('e2e-admin-token');
  page.once('dialog', dialog => dialog.accept('local-password'));
  await page.getByRole('button', { name: '创建签名身份' }).click();
  await expect(page.locator('#writeMode')).toContainText(`签名交易 · ${account}`);
  await expect(page.getByRole('button', { name: '创建签名身份' })).toBeEnabled();
  await expect(page.locator('#roomEntities')).toContainText('Mira');
  return Number(await page.locator('#actorInput').inputValue());
}

test('restores a stored identity as locked after a browser refresh', async ({ page }) => {
  const account = `refresh-identity-${Date.now()}`;
  const actorId = await createSignedIdentity(page, account);

  await page.reload();
  await page.locator('#actorInput').fill(String(actorId));
  await page.getByRole('button', { name: '连接节点' }).click();
  await expect(page.locator('#statusText')).toHaveText('节点在线');
  await expect(page.locator('#writeMode')).toContainText(`身份已锁定 · ${account}`);
  await expect(page.getByRole('button', { name: '解锁本地身份' })).toBeVisible();
  await expect(page.locator('#commandInput')).toBeEnabled();

  page.once('dialog', dialog => dialog.accept('local-password'));
  await page.getByRole('button', { name: '解锁本地身份' }).click();
  await expect(page.locator('#writeMode')).toContainText(`签名交易 · ${account}`);
  await page.locator('#roomEntities').getByRole('button', { name: /Mira/ }).click();
  await expect(page.locator('#receipt')).toContainText('SUCCESS');
});

test('prevents duplicate identity creation while the request is pending', async ({ page }) => {
  await page.goto('/');
  await page.getByRole('button', { name: '连接节点' }).click();
  await expect(page.locator('#statusText')).toHaveText('节点在线');
  await expect(page.getByRole('button', { name: '创建签名身份' })).toBeEnabled();
  let accountRequests = 0;
  let releaseAccount!: () => void;
  const accountRelease = new Promise<void>(resolve => { releaseAccount = resolve; });
  await page.route(rpcUrl, async route => {
    const request = route.request().postDataJSON();
    if (request.method === 'aomori_create_account') {
      accountRequests++;
      await accountRelease;
    }
    await route.continue();
  });
  const account = `duplicate-identity-${Date.now()}`;
  await page.locator('#accountInput').fill(account);
  await page.locator('#adminTokenInput').fill('e2e-admin-token');
  page.once('dialog', dialog => dialog.accept('local-password'));
  await page.getByRole('button', { name: '创建签名身份' }).click();
  await expect.poll(() => accountRequests).toBe(1);
  await expect(page.getByRole('button', { name: '创建签名身份' })).toBeDisabled();
  await page.getByRole('button', { name: '创建签名身份' }).click();
  expect(accountRequests).toBe(1);
  releaseAccount();
  await expect(page.getByRole('button', { name: '创建签名身份' })).toBeEnabled();
});

test('recovers controls after a signed transaction network failure', async ({ page }) => {
  const account = 'network-recovery-player';
  await createSignedIdentity(page, account);

  let failedSubmissions = 0;
  await page.route(rpcUrl, async route => {
    const request = route.request().postDataJSON();
    if (request.method === 'aomori_submit_transaction') {
      failedSubmissions++;
      await route.abort('failed');
    } else if (request.method === 'aomori_get_receipt') {
      expect(request.params.tx_id).toMatch(/^[0-9a-f]{64}$/);
      await route.fulfill({ status: 200, json: { jsonrpc: '2.0', id: request.id, result: { ok: true, tx_id: request.params.tx_id, state_root: 'reconciled-root', messages: ['查询到已提交交易'] } } });
    } else await route.continue();
  });

  await page.locator('#roomEntities').getByRole('button', { name: /Mira/ }).click();
  await expect(page.locator('#receipt')).toContainText('UNKNOWN');
  await expect(page.getByRole('button', { name: '查询交易结果' })).toBeEnabled();
  expect(failedSubmissions).toBe(1);
  await page.getByRole('button', { name: '查询交易结果' }).click();
  await expect(page.locator('#receipt')).toContainText('SUCCESS');
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

test('keeps an unknown receipt retryable when the node has not indexed it yet', async ({ page }) => {
  const account = `receipt-pending-${Date.now()}`;
  await createSignedIdentity(page, account);
  let receiptQueries = 0;
  await page.route(rpcUrl, async route => {
    const request = route.request().postDataJSON();
    if (request.method === 'aomori_submit_transaction') {
      await route.abort('failed');
      return;
    }
    if (request.method === 'aomori_get_receipt') {
      receiptQueries++;
      if (receiptQueries === 1) {
        await route.fulfill({ status: 200, json: { jsonrpc: '2.0', id: request.id, result: null } });
      } else {
        await route.fulfill({ status: 200, json: { jsonrpc: '2.0', id: request.id, result: { ok: true, tx_id: request.params.tx_id, state_root: 'delayed-root', messages: ['延迟回执已确认'] } } });
      }
      return;
    }
    await route.continue();
  });

  await page.locator('#roomEntities').getByRole('button', { name: /Mira/ }).click();
  await expect(page.locator('#receipt')).toContainText('UNKNOWN');
  await page.getByRole('button', { name: '查询交易结果' }).click();
  await expect(page.locator('#log')).toContainText('节点尚未返回该交易回执');
  await expect(page.locator('#receipt')).toContainText('UNKNOWN');
  await expect(page.getByRole('button', { name: '查询交易结果' })).toBeEnabled();
  await page.getByRole('button', { name: '查询交易结果' }).click();
  await expect(page.locator('#receipt')).toContainText('SUCCESS');
  expect(receiptQueries).toBe(2);
});

test('keeps an unknown receipt retryable after a receipt query failure', async ({ page }) => {
  const account = `receipt-failure-${Date.now()}`;
  await createSignedIdentity(page, account);
  let receiptQueries = 0;
  await page.route(rpcUrl, async route => {
    const request = route.request().postDataJSON();
    if (request.method === 'aomori_submit_transaction') {
      await route.abort('failed');
      return;
    }
    if (request.method === 'aomori_get_receipt') {
      receiptQueries++;
      await route.abort('failed');
      return;
    }
    await route.continue();
  });

  await page.locator('#roomEntities').getByRole('button', { name: /Mira/ }).click();
  await expect(page.locator('#receipt')).toContainText('UNKNOWN');
  await page.getByRole('button', { name: '查询交易结果' }).click();
  await expect(page.locator('#log')).toContainText('无法连接节点，请检查节点地址或网络连接');
  await expect(page.locator('#receipt')).toContainText('UNKNOWN');
  await expect(page.getByRole('button', { name: '查询交易结果' })).toBeEnabled();
  expect(receiptQueries).toBe(1);
});

test('restores the receipt query button after a timeout', async ({ page }) => {
  const account = `receipt-timeout-${Date.now()}`;
  await createSignedIdentity(page, account);
  let receiptQueries = 0;
  await page.route(rpcUrl, async route => {
    const request = route.request().postDataJSON();
    if (request.method === 'aomori_submit_transaction') {
      await route.abort('failed');
      return;
    }
    if (request.method === 'aomori_get_receipt') {
      receiptQueries++;
      await new Promise(() => undefined);
      return;
    }
    await route.continue();
  });

  await page.locator('#roomEntities').getByRole('button', { name: /Mira/ }).click();
  await expect(page.locator('#receipt')).toContainText('UNKNOWN');
  await page.getByRole('button', { name: '查询交易结果' }).click();
  await expect(page.locator('#log')).toContainText('RPC 请求超时，请检查节点连接', { timeout: 10_000 });
  await expect(page.locator('#receipt')).toContainText('UNKNOWN');
  await expect(page.getByRole('button', { name: '查询交易结果' })).toBeEnabled();
  expect(receiptQueries).toBe(1);
});

test('ignores duplicate receipt query clicks while the request is pending', async ({ page }) => {
  const account = `receipt-duplicate-${Date.now()}`;
  await createSignedIdentity(page, account);
  let receiptQueries = 0;
  let releaseReceipt!: () => void;
  const receiptRelease = new Promise<void>(resolve => { releaseReceipt = resolve; });
  await page.route(rpcUrl, async route => {
    const request = route.request().postDataJSON();
    if (request.method === 'aomori_submit_transaction') {
      await route.abort('failed');
      return;
    }
    if (request.method === 'aomori_get_receipt') {
      receiptQueries++;
      await receiptRelease;
      await route.fulfill({ status: 200, json: { jsonrpc: '2.0', id: request.id, result: { ok: true, tx_id: request.params.tx_id, state_root: 'duplicate-query-root' } } });
      return;
    }
    await route.continue();
  });

  await page.locator('#roomEntities').getByRole('button', { name: /Mira/ }).click();
  await expect(page.locator('#receipt')).toContainText('UNKNOWN');
  const queryButton = page.getByRole('button', { name: '查询交易结果' });
  await queryButton.click();
  await expect(queryButton).toBeDisabled();
  await queryButton.evaluate(button => button.click());
  expect(receiptQueries).toBe(1);
  releaseReceipt();
  await expect(page.locator('#receipt')).toContainText('SUCCESS');
});

test('discards an in-flight receipt query after switching RPC endpoints', async ({ page }) => {
  const account = `receipt-switch-${Date.now()}`;
  await createSignedIdentity(page, account);
  let releaseReceipt!: () => void;
  const receiptRelease = new Promise<void>(resolve => { releaseReceipt = resolve; });
  await page.route(rpcUrl, async route => {
    const request = route.request().postDataJSON();
    if (request.method === 'aomori_submit_transaction') {
      await route.abort('failed');
      return;
    }
    if (request.method === 'aomori_get_receipt') {
      await receiptRelease;
      await route.fulfill({ status: 200, json: { jsonrpc: '2.0', id: request.id, result: { ok: true, tx_id: request.params.tx_id, state_root: 'stale-root' } } });
      return;
    }
    await route.continue();
  });

  await page.locator('#roomEntities').getByRole('button', { name: /Mira/ }).click();
  await expect(page.locator('#receipt')).toContainText('UNKNOWN');
  await page.getByRole('button', { name: '查询交易结果' }).click();
  await expect(page.getByRole('button', { name: '查询交易结果' })).toBeDisabled();
  await page.locator('#rpcInput').fill('http://127.0.0.1:18094');
  await page.getByRole('button', { name: '连接节点' }).click();
  await expect(page.locator('#receipt')).toContainText('暂无交易');
  releaseReceipt();
  await expect(page.locator('#receipt')).toContainText('暂无交易');
  await expect(page.locator('#log')).not.toContainText('已查询到交易结果');
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

test('can retry identity import after node validation fails', async ({ page }) => {
  const account = 'import-retry-player';
  await createSignedIdentity(page, account);

  page.once('dialog', dialog => dialog.accept('backup-password'));
  const downloadPromise = page.waitForEvent('download');
  await page.getByRole('button', { name: '导出加密备份' }).click();
  const backupPath = await (await downloadPromise).path();
  expect(backupPath).toBeTruthy();
  await page.getByRole('button', { name: '删除本地身份' }).click();
  await expect(page.locator('#writeMode')).toHaveText('开发 command');

  let validationFailures = 0;
  await page.route(rpcUrl, async route => {
    const request = route.request().postDataJSON();
    if (request.method === 'aomori_get_account' && request.params.name === account && validationFailures === 0) {
      validationFailures++;
      await route.abort('failed');
      return;
    }
    await route.continue();
  });

  page.once('dialog', dialog => dialog.accept('backup-password'));
  await page.locator('#importIdentityFile').setInputFiles(backupPath!);
  await expect(page.locator('#log')).toContainText('无法连接节点，请检查节点地址或网络连接');
  await expect(page.locator('#writeMode')).toHaveText('开发 command');
  expect(validationFailures).toBe(1);

  await page.unroute(rpcUrl);
  page.once('dialog', dialog => dialog.accept('backup-password'));
  await page.locator('#importIdentityFile').setInputFiles(backupPath!);
  await expect(page.locator('#writeMode')).toContainText(`签名交易 · ${account}`);
  await expect(page.locator('#log')).toContainText(`已导入 ${account} 的签名身份`);
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
        await route.fulfill({ status: 200, json: { jsonrpc: '2.0', id: request.id, error: { code: -32003, message: 'invalid nonce: expected 1, got 0' } } });
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

test('stops after two nonce conflicts and restores failed receipt state', async ({ page }) => {
  const account = `nonce-conflict-${Date.now()}`;
  await createSignedIdentity(page, account);
  let submissions = 0;
  await page.route(rpcUrl, async route => {
    const request = route.request().postDataJSON();
    if (request.method === 'aomori_submit_transaction') {
      submissions++;
      await route.fulfill({ status: 200, json: { jsonrpc: '2.0', id: request.id, error: { code: -32003, message: 'invalid nonce: expected 1, got 0' } } });
      return;
    }
    await route.continue();
  });
  await page.locator('#roomEntities').getByRole('button', { name: /Mira/ }).click();
  await expect.poll(() => submissions).toBe(2);
  await expect(page.locator('#receipt')).toContainText('FAILED');
  await expect(page.locator('#commandInput')).toBeEnabled();
});

test('rejects malformed identity backups without changing identity state', async ({ page }) => {
  await page.goto('/');
  await page.locator('#importIdentityFile').setInputFiles({ name: 'broken.json', mimeType: 'application/json', buffer: Buffer.from(JSON.stringify({ format: 'aomori-ed25519-backup', version: 1, account: 'bad account' })) });
  await expect(page.locator('#log')).toContainText('不支持的身份备份格式');
  await expect(page.locator('#writeMode')).toHaveText('开发 command');
  await expect(page.locator('#unlockIdentityBtn')).toBeHidden();
});
