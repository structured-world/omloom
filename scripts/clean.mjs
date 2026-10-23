import { rmSync } from 'node:fs';
// Only this project's reproducible verification output, never dependencies/caches.
for (const path of ['dist', 'test-results', 'playwright-report']) {
  rmSync(new URL(`../${path}`, import.meta.url), { recursive: true, force: true });
}
