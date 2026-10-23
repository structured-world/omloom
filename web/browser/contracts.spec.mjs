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
      if (name === 'partial_refresh' || name === 'partial_import') {
        // Chromium must preserve mixed freshness and explicit presence; a UI
        // must not receive one new timestamp that implicitly freshens old A.
        const asset = JSON.parse(result.result.json).asset;
        const groups = name === 'partial_import' ? asset.imported : asset.observed;
        expect(groups).toHaveLength(2);
        const [old, fresh] = groups.map(group => group.evidence);
        expect(old.sourceId).toBe(fresh.sourceId);
        expect([old.observedAtMs, old.validUntilMs, fresh.observedAtMs, fresh.validUntilMs]).toEqual(['100', '200', '300', '400']);
        expect(old.fields.map(field => field.name)).toEqual(['name', 'delta']);
        expect(fresh.fields).toEqual([{ name: 'description', clear: 'NULL_VALUE' }, { name: 'enabled', flag: false }]);
        expect(asset.intended[0].field.text).toBe('accepted intent');
      }
      if (name === 'scan_full_page') {
        // The browser receives every row but each batch UUID appears only once
        // on the wire. Empty/missing per-row context cannot override the header.
        const scan = JSON.parse(result.result.json).scan;
        expect(scan.candidates).toHaveLength(128);
        for (const context of [scan.connectionId, scan.scanId]) {
          expect(Buffer.from(result.result.bytes).toString('hex').split(Buffer.from(context).toString('hex')).length - 1).toBe(1);
        }
      }
    }
  }
});
