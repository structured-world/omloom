import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { pathToFileURL } from 'node:url';
import { resolve } from 'node:path';
import { build } from 'esbuild';
import { parse, compileScript } from 'vue/compiler-sfc';

async function renderer(name) {
  const outfile = resolve(`dist/presentation/${name}.mjs`);
  await build({
    stdin: { contents: `import component from './web/app/${name}.vue'; import {createSSRApp} from 'vue'; import {renderToString} from 'vue/server-renderer'; export const render = props => renderToString(createSSRApp(component, props));`, resolveDir: process.cwd() },
    bundle: true, platform: 'node', format: 'esm', packages: 'external', outfile,
    plugins: [{ name: 'vue', setup(plugin) {
      plugin.onLoad({ filter: /\.vue$/ }, async ({ path }) => {
        const { descriptor } = parse(await readFile(path, 'utf8'), { filename: path });
        return { contents: compileScript(descriptor, { id: name, inlineTemplate: true }).content, loader: 'ts' };
      });
    } }],
  });
  globalThis.location = new URL('http://127.0.0.1');
  return (await import(pathToFileURL(outfile).href)).render;
}

test('interface address presentation retains unknown enum numbers without crashing', async () => {
  // A newer wire enum must not make the entire selected interface unreadable.
  const render = await renderer('CurrentAddresses');
  const output = await render({ value: { coverage: { source: 'fixture', observedAt: 100n, complete: true }, observations: [{ value: { address: '192.0.2.10', stability: 77, state: 88 }, observedAt: 100n }] } });
  assert.match(output, /192\.0\.2\.10/);
  assert.match(output, /unknown \(77\)/);
  assert.match(output, /unknown \(88\)/);
  const known = await render({ value: { coverage: { source: 'fixture', observedAt: 100n, complete: true }, observations: [{ value: { address: '192.0.2.10', stability: 1, state: 2 }, observedAt: 100n }] } });
  assert.match(known, /<small>stable · deprecated ·/);
});

test('endpoint presentation handles unknown enums and keeps wildcard semantics', async () => {
  // Future enum variants cannot hide the service's bind or break the detail panel.
  const render = await renderer('HostNetwork');
  const service = { assetId: 'service', incarnation: 'instance' };
  const evidence = { coverage: { source: 'fixture', observedAt: 100n }, services: [{ target: service, name: 'listener' }], observations: [
    { observedAt: 100n, value: { oneofKind: 'socket', socket: { service, protocol: 'tcp', family: 'IPv4', bind: '192.0.2.10', port: 443, addressStability: 77, addressState: 88 } } },
    { observedAt: 100n, value: { oneofKind: 'socket', socket: { service, protocol: 'tcp', family: 'IPv4', bind: '*', port: 8443, addressStability: 77, addressState: 88 } } },
  ] };
  const output = await render({ evidence });
  assert.match(output, /unknown \(77\).*unknown \(88\)/);
  assert.match(output, /all interfaces in this family/);
  assert.match(output, /192\.0\.2\.10:443/);
  assert.match(output, /listener/);
});
