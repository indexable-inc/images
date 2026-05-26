import { playwright } from '@vitest/browser-playwright';
import { defineConfig } from 'vitest/config';

export default defineConfig({
  test: {
    include: ['src/**/*.test.ts'],
    browser: {
      enabled: true,
      headless: true,
      provider: playwright({
        launchOptions: {
          // Required for chromium in the Nix sandbox: no /dev/shm,
          // no user namespaces.
          args: ['--no-sandbox', '--disable-dev-shm-usage']
        }
      }),
      instances: [{ browser: 'chromium' }]
    }
  }
});
