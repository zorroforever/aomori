import { expect, test } from '@playwright/test';

test('ignores duplicate command submissions while a command is pending', async ({ page }) => {
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

  await page.locator('#commandInput').fill('look');
  await page.locator('#commandForm').evaluate((form: HTMLFormElement) => form.requestSubmit());
  await expect.poll(() => lookRequests).toBe(1);
  expect(await page.locator('#commandInput').isDisabled()).toBe(true);
  await page.locator('#commandForm').evaluate((form: HTMLFormElement) => form.requestSubmit());
  expect(lookRequests).toBe(1);
  releaseLook();
  await expect(page.locator('#commandInput')).toBeEnabled();
});
