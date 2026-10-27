import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, readdir, readFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { withFieldHost } from '../browser/field-host.mjs';

test('field host cleans data after an early exit', async () => {
  // Startup failure must release the fixture directory before propagating its error.
  const parent = await mkdtemp(join(tmpdir(), 'omloom-host-lifecycle-'));
  try {
    await assert.rejects(withFieldHost(() => assert.fail('unexpected startup'), { parent, command: [process.execPath, '-e', 'process.exit(7)', '--'] }), /Host exited 7/);
    assert.deepEqual(await readdir(parent), []);
  } finally { await rm(parent, { recursive: true, force: true }); }
});

test('field host timeout cleans the process and data', async () => {
  // A child that never announces a URL must not survive the rejected startup.
  const parent = await mkdtemp(join(tmpdir(), 'omloom-host-lifecycle-'));
  const pidFile = join(parent, 'pid');
  let pid;
  try {
    const script = `require('node:fs').writeFileSync(${JSON.stringify(pidFile)}, String(process.pid)); setInterval(() => {}, 1000);`;
    await assert.rejects(withFieldHost(() => assert.fail('unexpected startup'), { parent, command: [process.execPath, '-e', script, '--'], timeoutMs: 1000 }), /Startup timed out/);
    pid = Number(await readFile(pidFile, 'utf8'));
    assert.throws(() => process.kill(pid, 0), { code: 'ESRCH' });
    assert.deepEqual(await readdir(parent), ['pid']);
  } finally {
    // Remove this suite's child even when an assertion fails before reading its pid.
    if (pid === undefined) {
      try { pid = Number(await readFile(pidFile, 'utf8')); }
      catch (e) { if (e.code !== 'ENOENT') throw e; }
    }
    if (pid !== undefined) { try { process.kill(pid, 'SIGKILL'); } catch (e) { if (e.code !== 'ESRCH') throw e; } }
    await rm(parent, { recursive: true, force: true });
  }
});

test('field host reports a missing executable and cleans its data', { timeout: 2000 }, async () => {
  // A failed spawn has no pid and never emits exit; cleanup must not wait for it.
  const parent = await mkdtemp(join(tmpdir(), 'omloom-host-lifecycle-'));
  try {
    await assert.rejects(withFieldHost(() => assert.fail('unexpected startup'), { parent, command: [join(parent, 'missing')], timeoutMs: 100 }), { code: 'ENOENT' });
    assert.deepEqual(await readdir(parent), []);
  } finally { await rm(parent, { recursive: true, force: true }); }
});
