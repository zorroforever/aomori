import { defineConfig, devices } from '@playwright/test';

export default defineConfig({
  testDir: './e2e',
  timeout: 30_000,
  expect: { timeout: 5_000 },
  use: {
    baseURL: 'http://127.0.0.1:15173',
    trace: 'retain-on-failure',
  },
  projects: [
    {
      name: 'chromium',
      use: { ...devices['Desktop Chrome'] },
    },
  ],
  webServer: [
    {
      command: 'bash -lc \'source "$HOME/.cargo/env" && rm -rf /tmp/aomori-e2e && AOMORI_ADMIN_TOKEN=e2e-admin-token AOMORI_CORS_ORIGINS=http://127.0.0.1:15173 cargo run -- --listen 127.0.0.1:18093 --data-dir /tmp/aomori-e2e --demo\'',
      cwd: '..',
      url: 'http://127.0.0.1:18093/health',
      timeout: 120_000,
      reuseExistingServer: false,
    },
    {
      command: 'VITE_AOMORI_RPC=http://127.0.0.1:18093 npm run dev -- --host 127.0.0.1 --port 15173',
      cwd: '.',
      url: 'http://127.0.0.1:15173',
      timeout: 30_000,
      reuseExistingServer: false,
    },
  ],
});
