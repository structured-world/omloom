import { test, expect } from '@playwright/test';
import { readFileSync } from 'node:fs';

test('generated contracts preserve Rust values in a real browser', async ({ page }) => {
  // This harness runs the shipped codec in Chromium, including TextDecoder and
  // BigInt; it does not substitute a fake product UI for installation acceptance.
  await page.goto('about:blank');
  await page.addScriptTag({ path: 'dist/browser-corpus.js' });
  const root = new URL('../../dist/corpus/', import.meta.url);
  const rows = readFileSync(new URL('manifest.tsv', root), 'utf8').trim().split('\n');
  for (const row of rows) {
    const [name, expected] = row.split('\t');
    const bytes = Array.from(readFileSync(new URL(`${name}.bin`, root)));
    const result = await page.evaluate(input => {
      try { return { result: globalThis.checkContract(input) }; }
      catch (error) { return { error: error.message }; }
    }, bytes);
    if (['Wire', 'UnknownField', 'DuplicateField', 'Limit'].includes(expected)) {
      expect(result.error, name).toBe(expected);
    } else {
      expect(result.result.bytes, name).toEqual(bytes);
      if (name === 'operation') {
        expect(result.result.state).toBe('RECONCILING');
        expect(JSON.parse(result.result.json).operation.revision).toBe('18446744073709551615');
      }
    }
  }
});
