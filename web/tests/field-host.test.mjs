import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, readdir, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { withFieldHost } from '../browser/field-host.mjs';

async function killRecordedChild(pidFile, known) {
  let pid = known;
  if (pid === undefined) {
    try { pid = Number(await readFile(pidFile, 'utf8')); }
    catch (e) { if (e.code !== 'ENOENT') throw e; }
  }
  if (!Number.isInteger(pid) || pid <= 0 || pid > 0x7fffffff) return;
  try { process.kill(pid, 'SIGKILL'); } catch (e) { if (e.code !== 'ESRCH') throw e; }
}

test('field host cleans data after an early exit', async () => {
  // Startup failure must release the fixture directory before propagating its error.
  const parent = await mkdtemp(join(tmpdir(), 'omloom-host-lifecycle-'));
  try {
    await assert.rejects(withFieldHost(() => assert.fail('unexpected startup'), { parent, command: [process.execPath, '-e', 'process.exit(7)', '--'] }), /Host exited 7/);
    assert.deepEqual(await readdir(parent), []);
  } finally { await rm(parent, { recursive: true, force: true }); }
});

test('field host timeout cleans the process and data', async t => {
  // A child that never announces a URL must not survive the rejected startup.
  const parent = await mkdtemp(join(tmpdir(), 'omloom-host-lifecycle-'));
  const pidFile = join(parent, 'pid');
  let pid;
  t.after(async () => {
    // Separate teardown preserves the original test failure and always removes its directory.
    try { await killRecordedChild(pidFile, pid); }
    finally { await rm(parent, { recursive: true, force: true }); }
  });
  const script = `require('node:fs').writeFileSync(${JSON.stringify(pidFile)}, String(process.pid)); setInterval(() => {}, 1000);`;
  await assert.rejects(withFieldHost(() => assert.fail('unexpected startup'), { parent, command: [process.execPath, '-e', script, '--'], timeoutMs: 1000 }), /Startup timed out/);
  pid = Number(await readFile(pidFile, 'utf8'));
  assert.throws(() => process.kill(pid, 0), { code: 'ESRCH' });
  assert.deepEqual(await readdir(parent), ['pid']);
});

test('recorded child cleanup ignores invalid and process-group PIDs', async t => {
  // Empty/partial pid files must never signal the test runner's process group.
  const parent = await mkdtemp(join(tmpdir(), 'omloom-child-pid-'));
  t.after(() => rm(parent, { recursive: true, force: true }));
  const calls = [];
  t.mock.method(process, 'kill', (...args) => { calls.push(args); return true; });
  const pidFile = join(parent, 'pid');
  for (const value of ['', '0', '-1', '1.5', 'not-a-pid', '4294967296']) {
    await writeFile(pidFile, value);
    await killRecordedChild(pidFile);
  }
  assert.deepEqual(calls, []);
  await writeFile(pidFile, '123');
  await killRecordedChild(pidFile);
  assert.deepEqual(calls, [[123, 'SIGKILL']]);
});

test('field host reports a missing executable and cleans its data', { timeout: 2000 }, async () => {
  // A failed spawn has no pid and never emits exit; cleanup must not wait for it.
  const parent = await mkdtemp(join(tmpdir(), 'omloom-host-lifecycle-'));
  try {
    await assert.rejects(withFieldHost(() => assert.fail('unexpected startup'), { parent, command: [join(parent, 'missing')], timeoutMs: 100 }), { code: 'ENOENT' });
    assert.deepEqual(await readdir(parent), []);
  } finally { await rm(parent, { recursive: true, force: true }); }
});

test('field host forces an unresponsive child to exit and preserves the test error', { timeout: 5000 }, async t => {
  // A child ignoring SIGINT must not hang teardown or replace the callback failure.
  const parent = await mkdtemp(join(tmpdir(), 'omloom-host-shutdown-'));
  const pidFile = join(parent, 'pid');
  let pid, rescue;
  t.after(async () => {
    clearTimeout(rescue);
    try {
      if (Number.isInteger(pid) && pid > 0) {
        try { process.kill(pid, 'SIGKILL'); } catch (e) { if (e.code !== 'ESRCH') throw e; }
      }
    } finally { await rm(parent, { recursive: true, force: true }); }
  });
  const script = `process.on('SIGINT', () => {}); require('node:fs').writeFileSync(${JSON.stringify(pidFile)}, String(process.pid)); console.log('http://127.0.0.1:1'); setInterval(() => {}, 1000);`;
  const failure = Error('callback failure');
  const started = performance.now();
  await assert.rejects(withFieldHost(async () => {
    pid = Number(await readFile(pidFile, 'utf8'));
    assert.ok(Number.isInteger(pid) && pid > 0);
    // Bound the deliberately broken implementation too, without leaving a child behind.
    rescue = setTimeout(() => process.kill(pid, 'SIGKILL'), 3000);
    throw failure;
  }, { parent, command: [process.execPath, '-e', script, '--'] }), e => e === failure);
  assert.ok(performance.now() - started < 2500, 'shutdown must finish before emergency cleanup');
  assert.throws(() => process.kill(pid, 0), { code: 'ESRCH' });
  assert.deepEqual(await readdir(parent), ['pid']);
});
