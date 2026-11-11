import { chromium, expect } from '@playwright/test';
import { buildSync } from 'esbuild';
import { createHash } from 'node:crypto';
import { execFileSync } from 'node:child_process';
import { createReadStream } from 'node:fs';
import { mkdir, readdir, writeFile } from 'node:fs/promises';
import { arch, platform, release } from 'node:os';
import { join } from 'node:path';
import { performance } from 'node:perf_hooks';
import { withFieldHost } from '../browser/field-host.mjs';
import { fixture } from '../browser/physical-scene-fixture.mjs';

// Run against a fresh production build and a disposable, generated inventory.
const output = process.argv[2];
if (!output) throw Error('Usage: node web/bench/physical-renderer.mjs <new-report-directory>');
await mkdir(output);
const sha256 = bytes => createHash('sha256').update(bytes).digest('hex');
const sources = [
  'web/bench/physical-renderer.mjs',
  'web/browser/physical-scene-fixture.mjs',
  'web/app/PhysicalScene.vue',
  'web/app/physical-scene-renderer.ts',
  'web/app/physical-scene-data.ts',
  'yarn.lock',
  'Cargo.lock',
];
const artifacts = [
  'target/debug/omloom-dev',
  'dist/app/index.html',
  ...(await readdir('dist/app/assets')).sort().map(name => join('dist/app/assets', name)),
];
const hashes = {};
for (const file of [...sources, ...artifacts]) {
  const hash = createHash('sha256');
  for await (const chunk of createReadStream(file)) hash.update(chunk);
  hashes[file] = hash.digest('hex');
}
const transform = buildSync({
  entryPoints: ['web/app/physical-scene-data.ts'], bundle: true, write: false,
  format: 'esm', platform: 'node', define: { 'location.origin': '"http://localhost"' },
});
const { sceneData } = await import(`data:text/javascript;base64,${Buffer.from(transform.outputFiles[0].contents).toString('base64')}`);
const browser = await chromium.launch({ channel: 'chrome', headless: true, args: ['--enable-precise-memory-info'] });
const report = {
  commit: execFileSync('git', ['rev-parse', 'HEAD'], { encoding: 'utf8' }).trim(),
  browser: browser.version(),
  platform: platform(), release: release(), arch: arch(),
  hashes,
  // Qualification budgets for one selected rack, on local desktop hardware.
  budgets: { coldMs: 1500, refreshMs: 750, interactionP95Ms: 50, idleDraws: 0 },
  scenes: [],
};

function quantiles(samples) {
  samples.sort((a, b) => a - b);
  return Object.fromEntries([50, 95, 99].map(p => [`p${p}`, samples[Math.ceil(samples.length * p / 100) - 1]]));
}

try {
  for (const pairs of [1, 12]) await withFieldHost(async url => {
    const { m, rpc, rack, host, unknown, ports, cables } = await fixture(url, pairs);
    const selection = [rack, host, unknown, ...ports];
    const { physical } = await rpc({ oneofKind: 'physical', physical: m.Physical.create({
      action: { oneofKind: 'scene', scene: { subjects: selection } },
    }) });
    const projection = m.PhysicalView.toBinary(physical);
    await writeFile(join(output, `projection-${pairs}.pb`), projection);
    const { boxes, lines } = sceneData(physical, rack);
    const geometry = JSON.stringify({ boxes: boxes.map(box => ({ ...box, port: box.kind === m.AssetKind.PORT })), lines });
    await writeFile(join(output, `geometry-${pairs}.json`), geometry);
    const page = await browser.newPage({ viewport: { width: 1440, height: 900 }, deviceScaleFactor: 1 });
    const errors = [];
    page.on('pageerror', error => errors.push(String(error)));
    try {
      await page.addInitScript(() => {
        let draws = 0;
        const contexts = new WeakMap();
        const getContext = HTMLCanvasElement.prototype.getContext;
        HTMLCanvasElement.prototype.getContext = function (...args) {
          const context = getContext.apply(this, args);
          if (args[0] === 'webgl2' && context) contexts.set(this, context);
          return context;
        };
        for (const name of ['drawArrays', 'drawElements', 'drawArraysInstanced', 'drawElementsInstanced']) {
          const original = WebGL2RenderingContext.prototype[name];
          WebGL2RenderingContext.prototype[name] = function (...args) {
            draws++;
            return original.apply(this, args);
          };
        }
        window.rendererEvidence = () => {
          const canvas = document.querySelector('.scene-stage canvas');
          const gl = contexts.get(canvas);
          const ext = gl?.getExtension('WEBGL_debug_renderer_info');
          return {
            draws, gpu: ext ? gl.getParameter(ext.UNMASKED_RENDERER_WEBGL) : null,
            version: gl?.getParameter(gl.VERSION),
            heapBytes: performance.memory?.usedJSHeapSize,
            framebuffer: canvas ? [canvas.width, canvas.height] : null,
          };
        };
      });
      const cold = performance.now();
      await page.goto(`${url}/#/?view=2&scope=${rack.assetId}&scopeInc=${rack.incarnation}`);
      await page.getByRole('button', { name: '3D scene', exact: true }).click();
      const scene = page.getByRole('region', { name: 'Physical 3D workspace' });
      await expect(scene).toContainText('3 recorded envelopes');
      await page.waitForFunction(() => window.rendererEvidence().draws > 0);
      const coldMs = performance.now() - cold;
      await scene.getByRole('button', { name: 'Installed server', exact: true }).click();
      await scene.getByRole('button', { name: 'Show components', exact: true }).click();
      await expect(scene).toContainText(`${ports.length + 3} recorded envelopes`);
      await expect(scene).toContainText(`${cables.length} two-end cable diagrams`);

      const interactions = await scene.getByRole('button', { name: 'Rotate right' }).evaluate(async button => {
        const samples = [];
        for (let i = 0; i < 40; i++) {
          const before = window.rendererEvidence().draws;
          const start = performance.now();
          button.click();
          await new Promise(requestAnimationFrame);
          if (window.rendererEvidence().draws === before) throw Error('Camera input did not draw');
          if (i >= 10) samples.push(performance.now() - start);
        }
        return samples;
      });
      const beforeRefresh = await page.evaluate(() => window.rendererEvidence().draws);
      const refresh = performance.now();
      await scene.getByRole('button', { name: 'Refresh scene' }).click();
      await expect(scene).toContainText(`${cables.length} two-end cable diagrams`);
      await page.waitForFunction(before => window.rendererEvidence().draws > before, beforeRefresh);
      const refreshMs = performance.now() - refresh;
      const gpu = await page.evaluate(() => window.rendererEvidence());
      const idleDraws = await page.evaluate(async () => {
        await new Promise(requestAnimationFrame);
        const before = window.rendererEvidence().draws;
        await new Promise(resolve => setTimeout(resolve, 1000));
        return window.rendererEvidence().draws - before;
      });
      const interactionMs = quantiles(interactions);
      report.scenes.push({
        pairs, subjects: selection.length, projectedCables: physical.twoEndCables.length,
        projectionBytes: projection.byteLength, projectionSha256: sha256(projection),
        geometrySha256: sha256(geometry),
        coldMs, refreshMs, interactionMs, idleDraws, ...gpu, errors,
      });
      const { budgets } = report;
      expect(errors).toEqual([]);
      expect(gpu.gpu, 'a hardware renderer must be identifiable').toBeTruthy();
      expect(gpu.gpu).not.toMatch(/swiftshader|llvmpipe|software/i);
      expect(coldMs).toBeLessThan(budgets.coldMs);
      expect(refreshMs).toBeLessThan(budgets.refreshMs);
      expect(interactionMs.p95).toBeLessThan(budgets.interactionP95Ms);
      expect(idleDraws).toBe(budgets.idleDraws);
    } finally {
      await page.close();
    }
  });
} catch (error) {
  report.error = String(error);
  throw error;
} finally {
  await browser.close();
  await writeFile(join(output, 'report.json'), JSON.stringify(report, null, 2) + '\n');
}
