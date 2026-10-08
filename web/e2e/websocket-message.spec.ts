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

test('retains only the latest 200 events without moving the cursor backwards', async ({ page }) => {
  let sendEvents!: () => void;
  await page.routeWebSocket('ws://127.0.0.1:18093/events', ws => {
    ws.connectToServer();
    sendEvents = () => {
      for (let id = 940001; id <= 940205; id++) {
        ws.send(JSON.stringify({ id, head: id, kind: 'retained_event', data: { id } }));
      }
      ws.send(JSON.stringify({ id: 940001, head: 940001, kind: 'old_duplicate', data: {} }));
    };
  });

  await page.goto('/');
  await page.getByRole('button', { name: '连接节点' }).click();
  await expect(page.locator('#statusText')).toHaveText('节点在线');
  sendEvents();
  await expect(page.locator('#eventList .event')).toHaveCount(200);
  await expect(page.locator('#eventCount')).toHaveText('200');
  await expect(page.locator('#eventList .event-id').first()).toHaveText('#940205');
  await expect(page.locator('#eventList .event-id').last()).toHaveText('#940006');
  await expect(page.locator('#eventList')).not.toContainText('old_duplicate');
  await expect.poll(() => page.evaluate(() => localStorage.getItem('aomori:event-cursor:http://127.0.0.1:18093'))).toBe('940205');
});

test('reconnects and replays events when the recovery buffer fills up', async ({ page }) => {
  let releaseRecovery!: () => void;
  const recoveryRelease = new Promise<void>(resolve => { releaseRecovery = resolve; });
  let firstRead!: () => void;
  const firstReadStarted = new Promise<void>(resolve => { firstRead = resolve; });
  let reads = 0;
  let connections = 0;
  let sendEvents!: () => void;
  await page.route('http://127.0.0.1:18093/rpc', async route => {
    const request = route.request().postDataJSON();
    if (request.method !== 'aomori_get_events') return route.continue();
    reads++;
    if (reads === 1) {
      firstRead();
      await recoveryRelease;
      await route.fulfill({ status: 200, json: { jsonrpc: '2.0', id: request.id, result: { events: [], next: 0, latest: 0 } } });
      return;
    }
    const events = Array.from({ length: 205 }, (_, index) => ({ id: 950001 + index, head: 950001 + index, kind: 'replayed_event', data: {} }));
    await route.fulfill({ status: 200, json: { jsonrpc: '2.0', id: request.id, result: { events, next: 950205, latest: 950205 } } });
  });
  await page.routeWebSocket('ws://127.0.0.1:18093/events', ws => {
    connections++;
    ws.connectToServer();
    if (connections === 1) sendEvents = () => {
      for (let id = 950001; id <= 950205; id++) {
        ws.send(JSON.stringify({ id, head: id, kind: 'buffered_event', data: {} }));
      }
    };
  });

  await page.goto('/');
  await page.getByRole('button', { name: '连接节点' }).click();
  await firstReadStarted;
  sendEvents();
  await page.waitForTimeout(250);
  releaseRecovery();
  await expect.poll(() => connections, { timeout: 10_000 }).toBe(2);
  await expect.poll(() => reads).toBe(2);
  await expect(page.locator('#eventList .event')).toHaveCount(200);
  await expect(page.locator('#eventList .event-id').first()).toHaveText('#950205');
  await expect(page.locator('#eventList .event-id').last()).toHaveText('#950006');
  await expect.poll(() => page.evaluate(() => localStorage.getItem('aomori:event-cursor:http://127.0.0.1:18093'))).toBe('950205');
});

test('renders recovered events before live events received during compensation', async ({ page }) => {
  let releaseRecovery!: () => void;
  let recoveryStarted!: () => void;
  let sendLiveEvent!: () => void;
  const recoveryRelease = new Promise<void>(resolve => { releaseRecovery = resolve; });
  const recoveryRequest = new Promise<void>(resolve => { recoveryStarted = resolve; });
  await page.route('http://127.0.0.1:18093/rpc', async route => {
    const request = route.request().postDataJSON();
    if (request.method !== 'aomori_get_events') return route.continue();
    recoveryStarted();
    await recoveryRelease;
    await route.fulfill({ status: 200, json: { jsonrpc: '2.0', id: request.id, result: {
      events: [
        { id: 930001, head: 930001, kind: 'recovered_first', data: {} },
        { id: 930002, head: 930002, kind: 'recovered_second', data: {} },
      ],
      next: 930002, latest: 930003,
    } } });
  });
  await page.routeWebSocket('ws://127.0.0.1:18093/events', ws => {
    ws.connectToServer();
    sendLiveEvent = () => ws.send(JSON.stringify({ id: 930003, head: 930003, kind: 'live_third', data: {} }));
  });

  await page.goto('/');
  await page.getByRole('button', { name: '连接节点' }).click();
  await recoveryRequest;
  sendLiveEvent();
  await page.waitForTimeout(100);
  await expect(page.locator('#eventList')).not.toContainText('live_third');
  releaseRecovery();
  for (const id of [930001, 930002, 930003]) {
    await expect(page.locator('#eventList .event-id').filter({ hasText: `#${id}` })).toHaveCount(1);
  }
  await expect.poll(() => page.evaluate(() => localStorage.getItem('aomori:event-cursor:http://127.0.0.1:18093'))).toBe('930003');
});

test('cancels event compensation rate-limit retry after switching RPC', async ({ page }) => {
  let eventReads = 0;
  await page.route('http://127.0.0.1:18093/rpc', async route => {
    const request = route.request().postDataJSON();
    if (request.method === 'aomori_get_events') {
      eventReads++;
      await route.fulfill({ status: 429, json: { jsonrpc: '2.0', id: request.id, error: { code: -32004, message: 'rate limit exceeded', data: { retry_after_ms: 2000 } } } });
      return;
    }
    await route.continue();
  });

  await page.goto('/');
  await page.getByRole('button', { name: '连接节点' }).click();
  await expect.poll(() => eventReads).toBe(1);
  await expect(page.getByRole('button', { name: '连接节点' })).toBeEnabled();
  await page.locator('#rpcInput').fill('http://127.0.0.1:19999');
  await page.getByRole('button', { name: '连接节点' }).click();
  await expect(page.locator('#statusText')).toHaveText('连接失败');
  await page.waitForTimeout(2200);
  expect(eventReads).toBe(1);
  await expect(page.locator('#log')).not.toContainText('事件补偿失败');
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
