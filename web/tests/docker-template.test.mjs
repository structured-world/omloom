import assert from 'node:assert/strict';
import { execFile } from 'node:child_process';
import { mkdtemp, readFile, rm } from 'node:fs/promises';
import { createServer } from 'node:http';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';
import { promisify } from 'node:util';

// Exercise Docker's actual formatter against a synthetic local API. No daemon,
// containers, user contexts or additional language toolchain are involved.
test('Docker inspect template renders selected network attachments', { timeout: 15_000 }, async () => {
  const source = await readFile(new URL('../../crates/omloom-observe/src/runtime.rs', import.meta.url), 'utf8');
  const templates = [...source.matchAll(/let template = r#"([\s\S]*?)"#;/g)];
  assert.equal(templates.length, 1, 'one production inspect template');
  const template = templates[0][1];
  const networks = {
    'lan "east"': { EndpointID: 'c'.repeat(64), IPAddress: '192.0.2.9', IPPrefixLen: 24, GlobalIPv6Address: '2001:db8::9', GlobalIPv6PrefixLen: 64 },
    isolated: { EndpointID: 'd'.repeat(64), IPAddress: '198.51.100.8', IPPrefixLen: 28, GlobalIPv6Address: '', GlobalIPv6PrefixLen: 0 },
  };
  const ids = ['a'.repeat(64), 'b'.repeat(64)];
  const rows = ids.map((Id, index) => ({
    Id, Name: `/fixture-${index}`, Created: '2026-10-30T00:00:00Z', Image: `sha256:${'e'.repeat(64)}`,
    Config: { Labels: { 'com.docker.compose.project': 'fixture', 'com.docker.compose.service': 'web', 'com.docker.compose.project.working_dir': '/workspace/fixture' } },
    NetworkSettings: { Networks: index === 0 ? Object.fromEntries(Object.entries(networks).map(([name, value]) => [name, { ...value, Gateway: '192.0.2.1', DriverOpts: { unused: 'must not be rendered' } }])) : {} },
  }));
  const requests = [];
  const engine = createServer((request, response) => {
    const path = new URL(request.url, 'http://fixture').pathname;
    if (path === '/_ping' && ['HEAD', 'GET'].includes(request.method)) {
      response.writeHead(200, { 'API-Version': '1.48', OSType: 'linux', 'Content-Type': 'text/plain' });
      response.end('OK');
      return;
    }
    const index = rows.findIndex(row => path === `/v1.48/containers/${row.Id}/json`);
    requests.push({ method: request.method, path });
    response.writeHead(index >= 0 && request.method === 'GET' ? 200 : 404, { 'Content-Type': 'application/json' });
    response.end(JSON.stringify(index >= 0 ? rows[index] : { message: 'Unexpected fixture request' }));
  });
  const config = await mkdtemp(join(tmpdir(), 'omloom-docker-template-'));
  try {
    await new Promise((resolve, reject) => { engine.once('error', reject); engine.listen(0, '127.0.0.1', resolve); });
    const env = { ...process.env };
    for (const name of Object.keys(env)) if (name.startsWith('DOCKER_')) delete env[name];
    // The synthetic engine speaks this explicit API version.
    env.DOCKER_API_VERSION = '1.48';
    env.NO_PROXY = '*';
    const { stdout } = await promisify(execFile)('docker', [
      '--config', config, '--host', `tcp://127.0.0.1:${engine.address().port}`,
      'inspect', '--type', 'container', '--format', template, ...ids,
    ], { env, timeout: 10_000, maxBuffer: 1_048_576 });
    const rendered = stdout.trim().split('\n').map(line => JSON.parse(line));
    assert.deepEqual(rendered, rows.map((row, index) => ({
      id: row.Id, name: row.Name, created: row.Created, image: row.Image,
      project: 'fixture', service: 'web', working_directory: '/workspace/fixture',
      networks: index === 0 ? networks : {},
    })));
    assert.deepEqual(requests, ids.map(id => ({ method: 'GET', path: `/v1.48/containers/${id}/json` })));
  } finally {
    if (engine.listening) {
      const closed = new Promise((resolve, reject) => engine.close(error => error ? reject(error) : resolve()));
      engine.closeAllConnections();
      await closed;
    }
    await rm(config, { recursive: true, force: true });
  }
});
