import { expect, test } from '@playwright/test';

test('locks command controls while look is pending', async ({ page }) => {
  await page.goto('/');
  await page.getByRole('button', { name: '连接节点' }).click();
  await expect(page.locator('#statusText')).toHaveText('节点在线');

  let lookRequests = 0;
  let releaseLook!: () => void;
  const lookRelease = new Promise<void>(resolve => { releaseLook = resolve; });
  await page.route('http://127.0.0.1:18093/rpc', async route => {
    const request = route.request().postDataJSON();
    if (request.method === 'aomori_query' && request.params.action === 'look') {
      lookRequests++;
      await lookRelease;
    }
    await route.continue();
  });

  await page.getByRole('button', { name: '查看' }).click();
  await expect.poll(() => lookRequests).toBe(1);
  await expect(page.getByRole('button', { name: '查看' })).toBeDisabled();
  await expect(page.locator('#commandInput')).toBeDisabled();
  await expect(page.locator('#commandForm button')).toBeDisabled();
  await page.locator('#commandForm').evaluate((form: HTMLFormElement) => form.requestSubmit());
  expect(lookRequests).toBe(1);

  releaseLook();
  await expect(page.getByRole('button', { name: '查看' })).toBeEnabled();
  await expect(page.locator('#commandInput')).toBeEnabled();
});
