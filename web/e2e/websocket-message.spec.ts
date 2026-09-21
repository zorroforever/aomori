import { expect, test } from '@playwright/test';

test('deduplicates repeated event ids while keeping later events visible', async ({ page }) => {
  await page.routeWebSocket('ws://127.0.0.1:18093/events', ws => {
    const server = ws.connectToServer();
    server.onMessage(() => undefined);
    setTimeout(() => {
      const duplicate = JSON.stringify({ id: 900001, head: 900001, kind: 'test_event', data: { value: 'duplicate' } });
      ws.send(duplicate);
      ws.send(duplicate);
      ws.send(JSON.stringify({ id: 900002, head: 900002, kind: 'next_event', data: { value: 'later' } }));
    }, 300);
  });

  await page.goto('/');
  await page.getByRole('button', { name: '连接节点' }).click();
  await expect(page.locator('#statusText')).toHaveText('节点在线');
  await expect(page.locator('#eventList')).toContainText('test_event');
  await expect(page.locator('#eventList')).toContainText('next_event');
  await expect(page.locator('#eventList .event-id').filter({ hasText: '#900001' })).toHaveCount(1);
  await expect(page.locator('#eventList .event-id').filter({ hasText: '#900002' })).toHaveCount(1);
});

test('keeps the event stream usable after compensation fails', async ({ page }) => {
  let compensationFailed = false;
  await page.route('http://127.0.0.1:18093/rpc', async route => {
    const request = route.request().postDataJSON();
    if (request.method === 'aomori_get_events' && compensationFailed) {
      await route.abort('failed');
      return;
    }
    await route.continue();
  });
  await page.routeWebSocket('ws://127.0.0.1:18093/events', ws => {
    const server = ws.connectToServer();
    server.onMessage(() => undefined);
    setTimeout(() => {
      compensationFailed = true;
      ws.send(JSON.stringify({ type: 'event_stream_lagged', missed: 1, last_event_id: 900010 }));
      setTimeout(() => ws.send(JSON.stringify({ id: 900011, head: 900011, kind: 'after_compensation_failure', data: { value: 'still-live' } })), 200);
    }, 300);
  });

  await page.goto('/');
  await page.getByRole('button', { name: '连接节点' }).click();
  await expect(page.locator('#statusText')).toHaveText('节点在线');
  await expect(page.locator('#log')).toContainText('事件补偿失败');
  await expect(page.locator('#eventList')).toContainText('after_compensation_failure');
  await expect(page.locator('#statusText')).toHaveText('节点在线');
});

test('ignores out-of-order events without moving the event cursor backwards', async ({ page }) => {
  await page.routeWebSocket('ws://127.0.0.1:18093/events', ws => {
    const server = ws.connectToServer();
    server.onMessage(() => undefined);
    setTimeout(() => {
      ws.send(JSON.stringify({ id: 910001, head: 910001, kind: 'ordered_event', data: { value: 'first' } }));
      ws.send(JSON.stringify({ id: 910003, head: 910003, kind: 'latest_event', data: { value: 'third' } }));
      ws.send(JSON.stringify({ id: 910002, head: 910002, kind: 'late_event', data: { value: 'second' } }));
    }, 300);
  });

  await page.goto('/');
  await page.getByRole('button', { name: '连接节点' }).click();
  await expect(page.locator('#statusText')).toHaveText('节点在线');
  await expect(page.locator('#eventList')).toContainText('ordered_event');
  await expect(page.locator('#eventList')).toContainText('latest_event');
  await expect(page.locator('#eventList')).not.toContainText('late_event');
  await expect.poll(async () => page.evaluate(() => Number(Object.entries(localStorage).find(([key]) => key.startsWith('aomori:event-cursor:'))?.[1]))).toBe(910003);
});

test('reports malformed event stream messages without breaking the connection', async ({ page }) => {
  await page.routeWebSocket('ws://127.0.0.1:18093/events', ws => {
    const server = ws.connectToServer();
    server.onMessage(() => undefined);
    setTimeout(() => ws.send('not-json'), 300);
  });

  await page.goto('/');
  await page.getByRole('button', { name: '连接节点' }).click();
  await expect(page.locator('#statusText')).toHaveText('节点在线');
  await expect(page.locator('#log')).toContainText('事件通道返回了无效 JSON');
  await expect(page.locator('#statusText')).toHaveText('节点在线');
});
