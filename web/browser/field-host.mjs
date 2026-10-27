import { spawn } from 'node:child_process';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

export async function withFieldHost(run, { command = ['target/debug/omloom-dev'], parent = tmpdir(), timeoutMs = 20_000 } = {}) {
  const data = await mkdtemp(join(parent, 'omloom-field-ui-'));
  let child;
  try {
    child = spawn(command[0], [...command.slice(1), '--data', data, '--ui', 'dist/app', '--fixture', 'operator'], { stdio: ['ignore', 'pipe', 'pipe'] });
    const url = await new Promise((resolve, reject) => {
      let log = ''; const timer = setTimeout(() => reject(Error(`Startup timed out: ${log}`)), timeoutMs);
      child.stdout.on('data', chunk => { log += chunk; const match = log.match(/http:\/\/127\.0\.0\.1:\d+/); if (match) { clearTimeout(timer); resolve(match[0]); } });
      child.stderr.on('data', chunk => { log += chunk; });
      child.once('error', error => { clearTimeout(timer); reject(error); });
      child.once('exit', code => { clearTimeout(timer); reject(Error(`Host exited ${code}: ${log}`)); });
    });
    return await run(url);
  }
  finally {
    try {
      if (child?.pid !== undefined && child.exitCode === null && child.signalCode === null) {
        const exit = new Promise(resolve => child.once('exit', resolve));
        child.kill('SIGINT');
        const force = setTimeout(() => child.kill('SIGKILL'), 1000);
        try { await exit; } finally { clearTimeout(force); }
      }
    } finally { await rm(data, { recursive: true, force: true }); }
  }
}
