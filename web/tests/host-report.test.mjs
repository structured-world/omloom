import { test } from 'node:test';
import assert from 'node:assert/strict';
import { buildSync } from 'esbuild';

const { outputFiles } = buildSync({ stdin: { contents: 'export { readHostReport } from "./web/app/host-report.ts"; export { Request, HostReport, HostImport } from "./web/src/generated/omloom/v1/workspace.ts"; export { AssetKind } from "./web/src/generated/omloom/v1/contracts.ts";', resolveDir: process.cwd() }, bundle: true, platform: 'node', format: 'esm', write: false });
const m = await import(`data:text/javascript;base64,${Buffer.from(outputFiles[0].contents).toString('base64')}`);
function frame(findings) {
  return m.Request.toBinary(m.Request.create({ command: { oneofKind: 'hostImport', hostImport: m.HostImport.create({ report: m.HostReport.create({ version: 1, platform: 'macos', observedAt: 100n, findings }) }) } }));
}

test('host file field amplification is refused before generated allocation', () => {
  // A small file can contain thousands of empty messages. The actual reflection
  // reader must not allocate those messages before the structural limit fires.
  const bytes = frame(Array.from({ length: 4097 }, () => ({})));
  assert.ok(bytes.byteLength < 262144);
  const original = m.Request.fromBinary;
  let calls = 0;
  m.Request.fromBinary = function (...args) { calls++; return original.apply(this, args); };
  try {
    let message;
    try { m.readHostReport(bytes); } catch (error) { message = error.message; }
    assert.equal(message, 'Limit');
    assert.equal(calls, 0);
  } finally { m.Request.fromBinary = original; }
});

test('native host file keeps its source time and selected candidate facts', () => {
  // File parsing cannot invent receiver freshness or adopt an identity.
  const value = m.readHostReport(frame([{ key: 'hardware', kind: m.AssetKind.DEVICE, fields: [{ name: 'name', value: { oneofKind: 'text', text: 'Native machine' } }] }]));
  assert.equal(value.observedAt, 100n);
  assert.equal(value.findings[0].key, 'hardware');
});
