import { test, expect } from '@playwright/test';
import { spawn } from 'node:child_process';
import { mkdtemp, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

async function host(directory, fixture = 'operator', grants) {
  const args = ['--data', directory, '--ui', 'dist/app', '--fixture', fixture];
  if (grants) args.push('--grants', grants);
  const child = spawn('target/debug/omloom-dev', args, { stdio: ['ignore', 'pipe', 'pipe'] });
  const url = await new Promise((resolve, reject) => {
    let output = '';
    const timer = setTimeout(() => { child.kill('SIGINT'); reject(Error(`Host startup timed out: ${output}`)); }, 20_000);
    child.stdout.on('data', chunk => { output += chunk; const match = output.match(/http:\/\/127\.0\.0\.1:\d+/); if (match) { clearTimeout(timer); resolve(match[0]); } });
    child.stderr.on('data', chunk => { output += chunk; });
    child.once('exit', code => { clearTimeout(timer); reject(Error(`Host exited ${code}: ${output}`)); });
  });
  return { url, async stop() { if (child.exitCode !== null || child.signalCode !== null) return; const exit = new Promise(resolve => child.once('exit', resolve)); child.kill('SIGINT'); await exit; } };
}
async function create(page, name, kind = 'DEVICE') {
  // These fixtures create independent roots. Child creation has its own scoped journey.
  const close = page.getByRole('button', { name: 'Close resource details', exact: true });
  if (await close.isVisible()) { await close.click(); await expect(page.getByRole('region', { name: 'Resource details' })).toBeHidden(); }
  await page.getByRole('button', { name: '+ Create resource', exact: true }).click();
  const dialog = page.getByRole('dialog');
  await dialog.getByLabel('Resource kind', { exact: true }).selectOption({ label: kind.toLowerCase().replaceAll('_', ' ') }, { timeout: 5000 });
  await dialog.getByLabel('Name', { exact: true }).fill(name, { timeout: 5000 });
  await dialog.getByRole('button', { name: 'Save resource', exact: true }).click();
  await expect(dialog).toBeHidden();
  await expect(page.getByRole('region', { name: 'Resource details' }).getByRole('heading', { name, exact: true })).toBeVisible();
  await expect(page.getByRole('region', { name: 'Resource details' })).toBeVisible();
  const hash = new URL(page.url()).hash;
  const params = new URLSearchParams(hash.slice(hash.indexOf('?') + 1));
  return { asset_id: params.get('asset'), incarnation: params.get('incarnation') };
}

test('network rename preserves scopes beyond the bounded options window', async ({ page }) => {
  // Real disk/API fixtures place accepted references after 1,000 choices; rename must never clear them.
  test.setTimeout(120_000);
  const { buildSync } = await import('esbuild');
  const { GrpcWebFetchTransport } = await import('@protobuf-ts/grpcweb-transport');
  const built = buildSync({ stdin: { contents: 'export * from "./web/src/generated/omloom/v1/workspace"; export {AssetKind} from "./web/src/generated/omloom/v1/contracts";', resolveDir: process.cwd() }, bundle: true, write: false, format: 'esm', platform: 'node' });
  const m = await import('data:text/javascript;base64,' + Buffer.from(built.outputFiles[0].contents).toString('base64'));
  const dir = await mkdtemp(join(tmpdir(), 'omloom-network-options-'));
  const server = await host(dir);
  try {
    const transport = new GrpcWebFetchTransport({ baseUrl: server.url, format: 'binary' });
    async function rpc(command) {
      const q = await transport.unary(m.Workspace.methods[0], m.Request.create({ command: { oneofKind: 'query', query: m.Query.create({ limit: 1 }) } }), transport.mergeOptions());
      return (await transport.unary(m.Workspace.methods[0], m.Request.create({ command, working: { expected: q.response.result.page.revision } }), transport.mergeOptions())).response.result;
    }
    async function object(kind, name) {
      return (await rpc({ oneofKind: 'write', write: m.Write.create({ kind, changes: [{ name: 'name', value: { oneofKind: 'text', text: name } }] }) })).mutation.targets[0];
    }
    const space = await object(m.AssetKind.ADDRESS_SPACE, 'A namespace');
    for (let offset = 0; offset < 1000; offset += 100) {
      const csv = 'external_id,name\n' + Array.from({ length: 100 }, (_, i) => `${offset+i},A option ${String(offset+i).padStart(4,'0')}`).join('\n');
      const preview = (await rpc({ oneofKind:'import', import:m.Import.create({ source:'option-fixture', kind:m.AssetKind.VRF, csv, mappings:[{ source:'name', field:'name' }] }) })).preview;
      expect(preview.id).toBeDefined();
      expect(preview.rows.every(row => row.change !== undefined)).toBe(true);
      await rpc({ oneofKind:'import', import:m.Import.create({ preview:preview.id }) });
    }
    const link = await object(m.AssetKind.L2_DOMAIN, 'Z selected L2');
    const routing = await object(m.AssetKind.VRF, 'Z selected VRF');
    const target = (await rpc({ oneofKind:'network', network:m.Network.create({ kind:m.AssetKind.IP_ADDRESS, name:'Scoped address', cidr:'192.0.2.10/32', addressSpace:space, link, routing }) })).mutation.targets[0];
    const url = `${server.url}/#/?view=1&asset=${target.assetId}&incarnation=${target.incarnation}`;
    for (const width of [1440, 390]) {
      await page.setViewportSize({ width, height:900 });
      await page.goto(url);
      await page.getByRole('button', { name:'Review network settings', exact:true }).click();
      const dialog = page.getByRole('dialog');
      await expect(dialog).toContainText('Showing a bounded selection');
      const name = `Renamed address ${width}`;
      await dialog.getByLabel('Name', { exact:true }).fill(name);
      await dialog.getByRole('button', { name:'Apply reviewed settings', exact:true }).click();
      await expect(dialog).toBeHidden();
      const item = (await rpc({ oneofKind:'read', read:m.Read.create({ target }) })).detail.item;
      const field = key => item.fields.find(f => f.name === key)?.value.text;
      expect(field('name')).toBe(name);
      expect(field('link_scope')).toBe(`${link.assetId}/${link.incarnation}`);
      expect(field('routing_context')).toBe(`${routing.assetId}/${routing.incarnation}`);
    }
  } finally { await server.stop(); await rm(dir, { recursive:true, force:true }); }
});

test('local network evidence keeps service ownership, coverage and phone navigation', async ({ page }) => {
  // Actual API/storage setup exercises the same inspector on desktop and phone, without personal fixtures.
  const { buildSync } = await import('esbuild');
  const { GrpcWebFetchTransport } = await import('@protobuf-ts/grpcweb-transport');
  const built = buildSync({ stdin: { contents: 'export * from "./web/src/generated/omloom/v1/workspace"; export {AssetKind} from "./web/src/generated/omloom/v1/contracts";', resolveDir: process.cwd() }, bundle: true, write: false, format: 'esm', platform: 'node' });
  const m = await import('data:text/javascript;base64,' + Buffer.from(built.outputFiles[0].contents).toString('base64'));
  const dir = await mkdtemp(join(tmpdir(), 'omloom-network-evidence-'));
  const server = await host(dir);
  try {
    const transport = new GrpcWebFetchTransport({ baseUrl: server.url, format: 'binary' });
    async function rpc(command, focus) {
      const q = await transport.unary(m.Workspace.methods[0], m.Request.create({ command: { oneofKind: 'query', query: m.Query.create({ limit: 1 }) } }), transport.mergeOptions());
      const { response } = await transport.unary(m.Workspace.methods[0], m.Request.create({ command, working: { roots: [], focus, expected: q.response.result.page.revision } }), transport.mergeOptions());
      return response.result;
    }
    async function object(kind, name, parent) {
      const result = await rpc({ oneofKind: 'write', write: m.Write.create({ kind, parent, changes: [{ name: 'name', value: { oneofKind: 'text', text: name } }] }) }, parent);
      return result.mutation.targets[0];
    }
    const device = await object(m.AssetKind.DEVICE, 'Observed host');
    const os = await object(m.AssetKind.SOFTWARE_INSTALLATION, 'Observed OS', device);
    const service = await object(m.AssetKind.SERVICE, 'Observed tunnel', os);
    const tunnel = await object(m.AssetKind.INTERFACE, 'tunnel-local', service);
    await rpc({oneofKind:'interfaceAddresses',interfaceAddresses:m.InterfaceAddresses.create({interface:tunnel,source:'Fixture collector',observedAt:1000n,complete:true,addresses:[{address:'192.0.2.50',stability:m.AddressStability.STABLE,state:m.AddressState.PREFERRED}]})},tunnel);
    const otherHost = await object(m.AssetKind.DEVICE, 'Other host');
    const otherOS = await object(m.AssetKind.SOFTWARE_INSTALLATION, 'Other OS', otherHost);
    const otherService = await object(m.AssetKind.SERVICE, 'Observed tunnel', otherOS);
    const otherInterface = await object(m.AssetKind.INTERFACE, 'other-tunnel', otherService);
    await object(m.AssetKind.PORT, 'Direct physical port', device);
    const board = await object(m.AssetKind.DEVICE, 'Installed board', device);
    await object(m.AssetKind.PORT, 'Nested physical port', board);
    await rpc({oneofKind:'interfaceAddresses',interfaceAddresses:m.InterfaceAddresses.create({interface:otherInterface,source:'Fixture collector',observedAt:1000n,complete:true,addresses:[{address:'198.51.100.50',stability:m.AddressStability.STABLE,state:m.AddressState.PREFERRED}]})},otherInterface);
    // Ordinary browsing selects a level on the server, including count and pagination.
    // Opening one host must not materialize components of other hosts or deeper boards.
    for (const width of [1440, 390]) {
      await page.setViewportSize({width,height:900});
      await page.goto(`${server.url}/#/?view=2`);
      await expect(page.getByRole('button',{name:'Observed host',exact:true})).toBeVisible();
      await expect(page.getByRole('button',{name:'Other host',exact:true})).toBeVisible();
      await expect(page.locator('tbody')).not.toContainText('Direct physical port');
      await expect(page.locator('tbody')).not.toContainText('Installed board');
      await expect(page.locator('tbody')).not.toContainText('Nested physical port');
      await page.getByRole('button',{name:'Observed host',exact:true}).click();
      await page.getByRole('button',{name:'Use as scope',exact:true}).click();
      await expect(page.locator('tbody')).toContainText('Direct physical port');
      await expect(page.locator('tbody')).toContainText('Installed board');
      await expect(page.locator('tbody')).not.toContainText('Nested physical port');
      await expect(page.locator('tbody')).not.toContainText('Other host');
      const requested = await rpc({oneofKind:'query',query:m.Query.create({view:m.View.HARDWARE,levelOnly:true,roots:[device],limit:25,includeContext:true})});
      expect(requested.page.total).toBe(2n);
      expect(requested.page.contexts.every(c=>c.addresses.length===0)).toBe(true);
    }
    // An unaddressed interface must not borrow another interface's addresses from its OS.
    const osInterface=await object(m.AssetKind.INTERFACE,'addressed sibling',os);
    await object(m.AssetKind.INTERFACE,'unaddressed sibling',os);
    await rpc({oneofKind:'interfaceAddresses',interfaceAddresses:m.InterfaceAddresses.create({interface:osInterface,source:'Fixture collector',observedAt:1000n,complete:true,addresses:[{address:'192.0.2.99',stability:m.AddressStability.STABLE,state:m.AddressState.PREFERRED}]})},osInterface);
    await page.goto(`${server.url}/#/?view=1&search=unaddressed`);
    const unaddressed=page.locator('tbody tr').filter({has:page.getByRole('button',{name:'unaddressed sibling',exact:true})});
    await expect(unaddressed).toContainText('Address not known');
    await expect(unaddressed).not.toContainText('192.0.2.99');
    await expect(unaddressed).not.toContainText('Host address');
    await object(m.AssetKind.VIRTUAL_MACHINE, 'Fixture guest');
    await page.goto(`${server.url}/#/?view=3`);
    await expect(page.getByRole('button', { name: 'Fixture guest', exact: true })).toBeVisible();
    await expect(page.getByRole('button', { name: 'Observed host', exact: true })).toHaveCount(0);
    await expect(page.getByRole('button', { name: 'Other host', exact: true })).toHaveCount(0);
    const categories=page.getByRole('navigation',{name:'Resource categories'});
    await categories.getByRole('button',{name:'Hypervisors',exact:true}).click();
    await expect(page.getByRole('button',{name:'Fixture guest',exact:true})).toHaveCount(0);
    await page.getByRole('button',{name:'+ Create resource',exact:true}).click();
    await expect(page.getByRole('dialog').getByLabel('Resource kind',{exact:true}).locator('option:checked')).toHaveText('device');
    await expect(page.getByRole('dialog').getByLabel('Virtualization role',{exact:true})).toHaveValue('VIRTUALIZATION_ROLE_HYPERVISOR');
    await page.getByRole('dialog').getByRole('button',{name:'Cancel',exact:true}).click();
    await categories.getByRole('button',{name:'Virtual machines',exact:true}).click();
    await expect(page.getByRole('button',{name:'Fixture guest',exact:true})).toBeVisible();
    await page.reload();
    await expect(categories.getByRole('button',{name:'Virtual machines',exact:true})).toHaveAttribute('aria-pressed','true');
    await page.goto(`${server.url}/#/?view=4&search=Observed%20tunnel`);
    await expect(page.getByRole('button', { name: 'Observed tunnel', exact: true })).toHaveCount(2);
    await expect(page.getByRole('button', { name: 'Open owner Observed host / Observed OS', exact: true })).toBeVisible();
    await expect(page.getByRole('button', { name: 'Open owner Other host / Other OS', exact: true })).toBeVisible();
    await expect(page.getByText('Address: 192.0.2.50', { exact: true })).toBeVisible();
    await expect(page.getByText('Address: 198.51.100.50', { exact: true })).toBeVisible();
    await page.goto(`${server.url}/#/?asset=${tunnel.assetId}&incarnation=${tunnel.incarnation}`);
    const current=page.getByRole('region',{name:'Current interface addresses'});
    await expect(current).toContainText('192.0.2.50');
    await expect(current).toContainText('stable · preferred');
    // A partial collection must leave the first address's date intact in the actual UI.
    await rpc({oneofKind:'interfaceAddresses',interfaceAddresses:m.InterfaceAddresses.create({interface:tunnel,source:'Fixture collector',observedAt:2000n,complete:false,addresses:[{address:'192.0.2.51',stability:m.AddressStability.STABLE,state:m.AddressState.PREFERRED}]})},tunnel);
    await page.reload();
    await expect(current.locator('li').filter({hasText:'192.0.2.50'}).locator('time')).toHaveAttribute('datetime',new Date(1000).toISOString());
    await expect(current.locator('li').filter({hasText:'192.0.2.51'}).locator('time')).toHaveAttribute('datetime',new Date(2000).toISOString());
    const listener=await object(m.AssetKind.SERVICE,'Plain listener',os);
    await rpc({ oneofKind: 'hostNetwork', hostNetwork: m.HostNetwork.create({ installation: os, source: 'Test collection', observedAt: 1000n, basis: 'test-boot', socketCoverage: 'Kernel verified; process coverage partial', routeCoverage: 'Local IPv4 table', applicationCoverage: 'verified', packetFilterCoverage: 'not run or blocked: permission denied', firewallEnabled: true, blockAll: false, sockets: [
      { service, protocol: 'udp', family: 'IPv4', bind: '*', port: 51820 },
      { service, protocol: 'tcp', family: 'IPv4', bind: '127.0.0.1', port: 9000 },
      { service:listener, protocol:'tcp', family:'IPv4', bind:'192.0.2.60', port:8080 },
      { service, protocol:'udp', family:'IPv6', bind:'*', port:51821 },
    ], routes: [{ destination: 'default', gateway: '192.0.2.1', interface: 'en0', flags: 'UGSc', family: 'IPv4', linkLayer: false }], rules: [{ subject: '/Applications/Example.app', action: 'allow incoming' }] }) }, os);
    const address = `${server.url}/#/?asset=${os.assetId}&incarnation=${os.incarnation}`;
    for (const width of [1440, 390]) {
      await page.setViewportSize({ width, height: 900 });
      await page.goto(`${server.url}/#/?view=4&search=Plain%20listener`);
      const listenerRow=page.locator('tbody tr').filter({has:page.getByRole('button',{name:'Plain listener',exact:true})});
      await expect(listenerRow).toContainText('192.0.2.60:8080/tcp');
      await expect(listenerRow).toContainText('Observed host / Observed OS');
      await page.goto(address);
      const section = page.getByRole('region', { name: 'Local network observations', exact: true });
      await expect(section).toContainText('*:51820');
      await expect(section).toContainText('*:51821');
      await expect(section).not.toContainText('[*]');
      const ownerBox = await section.getByRole('button', { name: 'Observed tunnel', exact: true }).first().boundingBox();
      expect(ownerBox.width).toBeGreaterThan(100);
      expect(ownerBox.height).toBeLessThan(80);
      await expect(section).not.toContainText('127.0.0.1:9000');
      await expect(section.getByLabel('Non-loopback binds only')).toHaveCount(0);
      await expect(section).not.toContainText('127.0.0.1:9000');
      await section.getByRole('button', { name: 'Routes (1)', exact: true }).click();
      await expect(section).toContainText('192.0.2.1');
      await section.getByRole('button', { name: 'Firewall (1)', exact: true }).click();
      await expect(section).toContainText('permission denied');
      await expect(section).toContainText('Effective per-port decision: unknown');
      await expect(section).toContainText('/Applications/Example.app');
      await section.getByRole('button', { name: 'Endpoints (3)', exact: true }).click();
      await section.getByRole('button', { name: 'Observed tunnel', exact: true }).first().click();
      await expect(page.getByRole('heading', { name: 'Observed tunnel', exact: true })).toBeVisible();
      await expect(page.getByRole('region', { name: 'Resource details' })).toContainText('tunnel-local');
      await expect(section.getByRole('button', { name: /^Routes/ })).toHaveCount(0);
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
      await page.screenshot({path:`test-results/network-service-${width}.png`,fullPage:true});
    }
  } finally { await server.stop(); await rm(dir, { recursive: true, force: true }); }
});

test('phone creates local components and returns through the original context', async ({ page }) => {
  // Child creation must not become a global peer or require upstream network knowledge.
  const dir = await mkdtemp(join(tmpdir(), 'omloom-local-phone-'));
  const server = await host(dir);
  try {
    page.setDefaultTimeout(5000);
    await page.setViewportSize({ width: 390, height: 844 });
    await page.goto(server.url);
    const vm = await create(page, 'Guest under review', 'VIRTUAL_MACHINE');
    await page.getByLabel('Resource perspective', { exact: true }).selectOption({ label: 'Network' }, { timeout: 5000 });
    await expect(page.getByRole('button', { name: 'Allocate IP / prefix', exact: true })).toHaveCount(0);
    await page.getByRole('button', { name: '+ Create resource', exact: true }).click();
    const dialog = page.getByRole('dialog');
    await expect(dialog).toContainText('Inside Guest under review');
    await expect(dialog.getByLabel('Resource kind', { exact: true }).locator('option').filter({ hasText: /^virtual machine$/ })).toHaveCount(0);
    await dialog.getByLabel('Resource kind', { exact: true }).selectOption({ label: 'interface' });
    await dialog.getByLabel('Name', { exact: true }).fill('eth0');
    await dialog.getByRole('button', { name: 'Save resource', exact: true }).click();
    await expect(dialog).toBeHidden();
    await expect(page.getByRole('region', { name: 'Resource details' })).toContainText('eth0');
    await page.getByRole('button', { name: '+ Create resource', exact: true }).click();
    await dialog.getByLabel('Resource kind', { exact: true }).selectOption({ label: 'address reference' });
    await dialog.getByLabel('IP address', { exact: true }).fill('192.0.2.10');
    await expect(dialog.getByLabel('Address space', { exact: true })).toHaveCount(0);
    await dialog.getByRole('button', { name: 'Save known address', exact: true }).click();
    await expect(dialog).toBeHidden();
    await expect(page.getByRole('region', { name: 'Resource details' })).toContainText('192.0.2.10');
    await page.reload();
    await expect(page.getByRole('region', { name: 'Resource details' })).toContainText('192.0.2.10');
    await page.getByRole('button', { name: 'Close resource details', exact: true }).click();
    await expect(page.getByRole('region', { name: 'Resource details' })).toContainText('eth0');
    await page.getByRole('button', { name: 'Close resource details', exact: true }).click();
    expect(new URLSearchParams(new URL(page.url()).hash.split('?')[1]).get('asset')).toBe(vm.asset_id);
    await expect(page.getByRole('region', { name: 'Resource details' })).toContainText('Guest under review');
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
    await page.screenshot({ path: 'test-results/local-components-phone.png', fullPage: true });
  } finally { await server.stop(); await rm(dir, { recursive: true, force: true }); }
});

test('inventory choices apply together, cancel cleanly and keep the list compact', async ({ page }) => {
  // Native multi-selects clipped options and consumed the search area; choices need a deliberate review step.
  const dir = await mkdtemp(join(tmpdir(), 'omloom-choices-ui-'));
  const server = await host(dir);
  try {
    await page.setViewportSize({ width: 1024, height: 768 });
    await page.goto(server.url);
    await create(page, 'Rack host');
    await create(page, 'Build guest', 'VIRTUAL_MACHINE');
    await page.getByRole('button', { name: 'Close resource details', exact: true }).click();
    await page.getByRole('button', { name: 'Resource kinds', exact: true }).click();
    const dialog = page.getByRole('dialog');
    await dialog.getByRole('checkbox', { name: 'device', exact: true }).check();
    await dialog.getByRole('button', { name: 'Cancel', exact: true }).click();
    await expect(page.getByRole('button', { name: 'Build guest', exact: true })).toBeVisible();
    await page.getByRole('button', { name: 'Resource kinds', exact: true }).click();
    await dialog.getByRole('checkbox', { name: 'device', exact: true }).check();
    await dialog.getByRole('button', { name: 'Apply selection', exact: true }).click();
    await expect(page.getByRole('button', { name: 'Build guest', exact: true })).toHaveCount(0);
    await expect(page.getByRole('button', { name: 'Rack host', exact: true })).toBeVisible();
    await page.getByRole('button', { name: 'Columns', exact: true }).click();
    await dialog.getByRole('checkbox', { name: 'Serial number', exact: true }).check();
    await dialog.getByRole('button', { name: 'Apply selection', exact: true }).click();
    await expect(page.getByRole('columnheader', { name: 'serial', exact: true })).toBeVisible();
    await page.getByRole('button', { name: 'Columns', exact: true }).click();
    for (const name of ['Name', 'Description', 'Serial number']) await dialog.getByRole('checkbox', { name, exact: true }).uncheck();
    await expect(dialog.getByRole('button', { name: 'Apply selection', exact: true })).toBeDisabled();
    await page.keyboard.press('Escape');
    await expect(page.getByRole('button', { name: 'Columns', exact: true })).toBeFocused();
    await expect(page.locator('select[multiple]')).toHaveCount(0);
    const searchBox = await page.getByLabel('Search inventory').boundingBox();
    const table = await page.locator('table').boundingBox();
    expect(searchBox.width).toBeGreaterThan(280);
    expect(table.y).toBeLessThan(380);
    await page.setViewportSize({ width: 1440, height: 900 });
    await page.getByRole('button', { name: 'Rack host', exact: true }).click();
    await expect(page.getByRole('region', { name: 'Resource details' })).toBeVisible();
    const inventory = await page.getByRole('region', { name: 'Inventory', exact: true }).boundingBox();
    const detail = await page.getByRole('region', { name: 'Resource details' }).boundingBox();
    expect(detail.x).toBeGreaterThanOrEqual(inventory.x + inventory.width);
    await page.screenshot({ path: 'test-results/inventory-wide-inspector.png', fullPage: true });
  } finally { await server.stop(); await rm(dir, { recursive: true, force: true }); }
});

test('broader connections page beyond 25 peers and remove a later relationship', async ({ page }) => {
  // A focused inspector hides peer links; the broader dialog must expose every authorized page.
  const { buildSync } = await import('esbuild');
  const { GrpcWebFetchTransport } = await import('@protobuf-ts/grpcweb-transport');
  const built=buildSync({stdin:{contents:'export * from "./web/src/generated/omloom/v1/workspace"; export {AssetKind,RelationKind} from "./web/src/generated/omloom/v1/contracts";',resolveDir:process.cwd()},bundle:true,write:false,format:'esm',platform:'node'});
  const m=await import('data:text/javascript;base64,'+Buffer.from(built.outputFiles[0].contents).toString('base64'));
  const dir=await mkdtemp(join(tmpdir(),'omloom-peer-pages-')); const server=await host(dir);
  try {
    const transport=new GrpcWebFetchTransport({baseUrl:server.url,format:'binary'});
    async function rpc(command) {
      const q=(await transport.unary(m.Workspace.methods[0],m.Request.create({command:{oneofKind:'query',query:m.Query.create({limit:1})}}),transport.mergeOptions())).response.result;
      return (await transport.unary(m.Workspace.methods[0],m.Request.create({command,working:{expected:q.page.revision}}),transport.mergeOptions())).response.result;
    }
    async function object(name) { return (await rpc({oneofKind:'write',write:m.Write.create({kind:m.AssetKind.SERVICE,changes:[{name:'name',value:{oneofKind:'text',text:name}}]})})).mutation.targets[0]; }
    const anchor=await object('Application');
    for(let i=0;i<30;i++) {
      const peer=await object(`Peer ${i}`);
      await rpc({oneofKind:'relation',relation:m.Relationship.create({relation:{from:anchor,to:peer,kind:m.RelationKind.DEPENDS_ON}})});
    }
    for(const [index,width] of [1440,390].entries()) {
      const first=(await rpc({oneofKind:'read',read:m.Read.create({target:anchor})})).detail;
      expect(first.relations).toHaveLength(25);
      const second=(await rpc({oneofKind:'read',read:m.Read.create({target:anchor,relationsCursor:first.relationsNext})})).detail;
      const relation=second.relations[0];
      const item=second.related.find(i=>i.target.assetId===relation.to.assetId);
      const name=item.fields.find(f=>f.name==='name').value.text;
      await page.setViewportSize({width,height:900});
      await page.goto(`${server.url}/#/?asset=${anchor.assetId}&incarnation=${anchor.incarnation}`);
      await page.getByRole('button',{name:'Connections in broader scope',exact:true}).click();
      const dialog=page.getByRole('dialog',{name:'Connections in broader scope',exact:true});
      await dialog.getByRole('button',{name:'Review existing connections',exact:true}).click();
      await expect(dialog.locator('.connection-list li')).toHaveCount(25);
      await expect(dialog.getByRole('button',{name:'Next connections',exact:true})).toBeVisible();
      await dialog.getByRole('button',{name:'Next connections',exact:true}).click();
      await expect(dialog.locator('.connection-list li')).toHaveCount(5-index);
      await dialog.locator('.connection-list li').filter({has:page.getByText(name,{exact:true})}).getByRole('button',{name:'Review removal',exact:true}).click();
      await dialog.getByRole('button',{name:'Remove reviewed relationship',exact:true}).click();
      await expect(dialog).toBeHidden();
      const remaining=(await rpc({oneofKind:'read',read:m.Read.create({target:anchor,relationsCursor:undefined})})).detail;
      const last=(await rpc({oneofKind:'read',read:m.Read.create({target:anchor,relationsCursor:remaining.relationsNext})})).detail;
      const all=[...remaining.relations,...last.relations];
      expect(all).toHaveLength(29-index);
      expect(all.some(r=>r.relationId===relation.relationId)).toBe(false);
    }
  } finally { await server.stop(); await rm(dir,{recursive:true,force:true}); }
});

test('peer connection review preserves the resource context through edit and removal', async ({ page }) => {
  // Local details must never silently expose peers or edit an unseen old endpoint.
  const dir = await mkdtemp(join(tmpdir(), 'omloom-peer-ui-')); const server = await host(dir);
  try {
    await page.goto(server.url);
    await create(page,'Application','SERVICE');
    const backup = await create(page,'Backup','SERVICE');
    const archive = await create(page,'Archive','SERVICE');
    await page.getByRole('button',{name:'Close resource details',exact:true}).click();
    await page.getByLabel('Search inventory').fill('Application'); await page.getByRole('button',{name:'Search',exact:true}).click();
    await page.getByRole('button',{name:'Application',exact:true}).click(); const original = page.url();
    await page.getByRole('button',{name:'Connections in broader scope',exact:true}).click();
    const dialog = page.getByRole('dialog');
    await dialog.getByRole('button',{name:'Choose connection endpoints',exact:true}).click();
    await dialog.getByLabel('Other endpoint',{exact:true}).selectOption(`${backup.asset_id}/${backup.incarnation}`);
    await dialog.getByRole('button',{name:'Review connection',exact:true}).click();
    await dialog.getByLabel('Relationship',{exact:true}).selectOption({label:'depends on'});
    await dialog.getByRole('button',{name:'Back',exact:true}).click();
    await expect(dialog.getByLabel('Other endpoint',{exact:true})).toHaveValue(`${backup.asset_id}/${backup.incarnation}`);
    await dialog.getByRole('button',{name:'Review connection',exact:true}).click();
    await dialog.getByRole('button',{name:'Add relationship',exact:true}).click(); await expect(dialog).toBeHidden();
    await expect(page).toHaveURL(original);
    await expect(page.getByRole('region',{name:'Resource details'})).not.toContainText('Backup');
    await page.getByRole('button',{name:'Connections in broader scope',exact:true}).click();
    await dialog.getByRole('button',{name:'Review existing connections',exact:true}).click();
    await expect(dialog).toContainText('Backup');
    await dialog.getByRole('button',{name:'Change connection',exact:true}).click();
    await dialog.getByLabel('Other endpoint',{exact:true}).selectOption(`${archive.asset_id}/${archive.incarnation}`);
    await dialog.getByRole('button',{name:'Review connection',exact:true}).click();
    await dialog.getByRole('button',{name:'Save reviewed relationship',exact:true}).click(); await expect(dialog).toBeHidden();
    await page.getByRole('button',{name:'Connections in broader scope',exact:true}).click();
    await dialog.getByRole('button',{name:'Review existing connections',exact:true}).click();
    await expect(dialog).toContainText('Archive'); await expect(dialog).not.toContainText('Backup');
    await dialog.getByRole('button',{name:'Review removal',exact:true}).click();
    await dialog.getByRole('button',{name:'Remove reviewed relationship',exact:true}).click(); await expect(dialog).toBeHidden();
    await page.reload(); await expect(page).toHaveURL(original);
    await page.getByRole('button',{name:'Connections in broader scope',exact:true}).click();
    await dialog.getByRole('button',{name:'Review existing connections',exact:true}).click();
    await expect(dialog).toContainText('No connections are visible');
    await page.screenshot({path:'test-results/peer-context-desktop.png',fullPage:true});
  } finally { await server.stop(); await rm(dir,{recursive:true,force:true}); }
});

test('assigned allocations identify interfaces and services on desktop and phone', async ({ page }) => {
  // A real participant assignment must supply row hints without an address reference or observation.
  const { buildSync } = await import('esbuild');
  const { GrpcWebFetchTransport } = await import('@protobuf-ts/grpcweb-transport');
  const built = buildSync({ stdin:{ contents:'export * from "./web/src/generated/omloom/v1/workspace"; export {AssetKind} from "./web/src/generated/omloom/v1/contracts";', resolveDir:process.cwd() }, bundle:true, write:false, format:'esm', platform:'node' });
  const m = await import('data:text/javascript;base64,' + Buffer.from(built.outputFiles[0].contents).toString('base64'));
  const dir = await mkdtemp(join(tmpdir(), 'omloom-assignment-hints-')); const server = await host(dir);
  try {
    const transport = new GrpcWebFetchTransport({ baseUrl:server.url, format:'binary' });
    async function rpc(command, focus) {
      const q = (await transport.unary(m.Workspace.methods[0], m.Request.create({command:{oneofKind:'query',query:m.Query.create({limit:1})}}),transport.mergeOptions())).response.result;
      return (await transport.unary(m.Workspace.methods[0],m.Request.create({command,working:{focus,expected:q.page.revision}}),transport.mergeOptions())).response.result;
    }
    async function object(kind,name,parent) { return (await rpc({oneofKind:'write',write:m.Write.create({kind,parent,changes:[{name:'name',value:{oneofKind:'text',text:name}}]})},parent)).mutation.targets[0]; }
    const space=await object(m.AssetKind.ADDRESS_SPACE,'Assignment namespace');
    const service=await object(m.AssetKind.SERVICE,'Assigned service');
    const iface=await object(m.AssetKind.INTERFACE,'Assigned interface',service);
    const address=(await rpc({oneofKind:'network',network:m.Network.create({kind:m.AssetKind.IP_ADDRESS,name:'Canonical allocation',cidr:'192.0.2.51/32',addressSpace:space})})).mutation.targets[0];
    const detail=(await rpc({oneofKind:'read',read:m.Read.create({target:address})})).detail;
    await rpc({oneofKind:'member',member:m.Member.create({address,interface:iface,expected:detail.item.revision})});
    for (const width of [1440,390]) {
      await page.setViewportSize({width,height:900});
      for (const name of ['Assigned service','Assigned interface']) {
        await page.goto(`${server.url}/#/?view=0&search=${encodeURIComponent(name)}`);
        const row=page.getByRole('row').filter({has:page.getByRole('button',{name,exact:true})});
        await expect(row).toContainText('192.0.2.51');
        await page.getByRole('button',{name,exact:true}).click();
        await expect(page.getByRole('region',{name:'Resource details'}).getByRole('heading',{name,exact:true})).toBeVisible();
      }
    }
  } finally { await server.stop(); await rm(dir,{recursive:true,force:true}); }
});

test('VRRP reconciliation submits intended priority for existing and prepared allocations', async ({ page }) => {
  // All three reconciliation paths commit real membership, including a prepared address in an existing group.
  const { buildSync } = await import('esbuild');
  const { GrpcWebFetchTransport } = await import('@protobuf-ts/grpcweb-transport');
  const built = buildSync({ stdin:{ contents:'export * from "./web/src/generated/omloom/v1/workspace"; export {AssetKind} from "./web/src/generated/omloom/v1/contracts";', resolveDir:process.cwd() }, bundle:true, write:false, format:'esm', platform:'node' });
  const m = await import('data:text/javascript;base64,' + Buffer.from(built.outputFiles[0].contents).toString('base64'));
  const dir = await mkdtemp(join(tmpdir(), 'omloom-vrrp-binding-')); const server = await host(dir);
  try {
    const transport = new GrpcWebFetchTransport({ baseUrl:server.url, format:'binary' });
    async function rpc(command) {
      const q = (await transport.unary(m.Workspace.methods[0], m.Request.create({command:{oneofKind:'query',query:m.Query.create({limit:1})}}),transport.mergeOptions())).response.result;
      return (await transport.unary(m.Workspace.methods[0],m.Request.create({command,working:{expected:q.page.revision}}),transport.mergeOptions())).response.result;
    }
    async function object(kind,name) { return (await rpc({oneofKind:'write',write:m.Write.create({kind,changes:[{name:'name',value:{oneofKind:'text',text:name}}]})})).mutation.targets[0]; }
    const space=await object(m.AssetKind.ADDRESS_SPACE,'VRRP namespace');
    const link=await object(m.AssetKind.L2_DOMAIN,'VRRP link');
    const iface=await object(m.AssetKind.INTERFACE,'Gateway eth0');
    const existing=(await rpc({oneofKind:'network',network:m.Network.create({kind:m.AssetKind.IP_ADDRESS,name:'Existing VRRP',cidr:'192.0.2.81/32',addressSpace:space,link,sharing:m.SharedUse.VRRP,vrid:41,groupName:'Existing gateway group'})})).mutation.targets[0];
    const group=(await rpc({oneofKind:'read',read:m.Read.create({target:existing})})).detail.group;
    for (const [index,mode] of ['existing','new','group'].entries()) {
      const address=`192.0.2.${81+index}`;
      const fact=(await rpc({oneofKind:'address',address:m.Address.create({address,meaning:m.AddressMeaning.CONFIGURED})})).mutation.targets[0];
      await page.setViewportSize({width:index===0?1440:390,height:900});
      await page.goto(`${server.url}/#/?asset=${fact.assetId}&incarnation=${fact.incarnation}`);
      await page.getByRole('button',{name:'Resolve address in broader scope',exact:true}).click();
      const dialog=page.getByRole('dialog',{name:'Resolve address in broader scope',exact:true});
      await dialog.getByRole('button',{name:'Choose allocation context',exact:true}).click();
      if (mode==='existing') {
        await dialog.getByLabel('Canonical allocation',{exact:true}).selectOption(`${existing.assetId}/${existing.incarnation}`);
        await dialog.getByRole('button',{name:'Review address mapping',exact:true}).click();
      } else {
        await dialog.getByRole('button',{name:'Prepare new allocation',exact:true}).click();
        const allocation=page.getByRole('dialog',{name:'Allocate network resource',exact:true});
        await allocation.getByLabel('Name',{exact:true}).fill(`Prepared ${mode}`);
        await allocation.getByLabel('Address / prefix',{exact:true}).fill(`${address}/32`);
        await allocation.getByLabel('Address space',{exact:true}).selectOption(space.assetId);
        await allocation.getByLabel('Address use',{exact:true}).selectOption(String(m.SharedUse.VRRP));
        if (mode==='group') {
          await allocation.getByLabel('Group selection',{exact:true}).selectOption('existing');
          await allocation.getByLabel('Shared address group',{exact:true}).selectOption(group.target.assetId);
        } else {
          await allocation.getByLabel('L2 domain',{exact:true}).selectOption(link.assetId);
          await allocation.getByLabel('VRID',{exact:true}).fill('42');
          await allocation.getByLabel('New group name',{exact:true}).fill('New gateway group');
        }
        await allocation.getByRole('button',{name:'Review allocation',exact:true}).click();
        await expect(allocation).toBeHidden();
      }
      await dialog.getByLabel('Intended assignment',{exact:true}).selectOption(`${iface.assetId}/${iface.incarnation}`);
      await expect(dialog.getByLabel('Intended priority',{exact:true})).toBeVisible();
      await dialog.getByLabel('Intended priority',{exact:true}).fill(String(150+index));
      await dialog.getByRole('button',{name:'Apply reviewed mapping',exact:true}).click();
      await expect(dialog).toBeHidden();
      const item=(await rpc({oneofKind:'read',read:m.Read.create({target:fact})})).detail.item;
      const binding=item.fields.find(f=>f.name==='binding').value.text.split('/');
      const detail=(await rpc({oneofKind:'read',read:m.Read.create({target:{assetId:binding[0],incarnation:binding[1]}})})).detail;
      expect(detail.participants).toHaveLength(1);
      expect(detail.participants[0].interface).toEqual(iface);
      expect(detail.participants[0].priority).toBe(150+index);
    }
  } finally { await server.stop(); await rm(dir,{recursive:true,force:true}); }
});

test('address mapping reviews a new allocation and commits its explicit assignment together', async ({ page }) => {
  // Preparing a new canonical address must not allocate before the final mapping review.
  const dir = await mkdtemp(join(tmpdir(), 'omloom-binding-ui-')); let server = await host(dir);
  try {
    await page.goto(server.url); await create(page,'Real namespace','ADDRESS_SPACE');
    const iface = await create(page,'Guest eth0','INTERFACE');
    await page.getByRole('button',{name:'+ Create resource',exact:true}).click();
    let dialog = page.getByRole('dialog');
    await dialog.getByLabel('Resource kind',{exact:true}).selectOption({label:'address reference'});
    await dialog.getByLabel('IP address',{exact:true}).fill('192.0.2.80');
    await dialog.getByRole('button',{name:'More known details',exact:true}).click();
    await dialog.getByLabel('Address meaning change',{exact:true}).selectOption('set');
    await dialog.getByLabel('Address meaning',{exact:true}).selectOption({label:'configured'});
    await dialog.getByRole('button',{name:'Save known address',exact:true}).click(); await expect(dialog).toBeHidden();
    await page.getByRole('button',{name:'Resolve address in broader scope',exact:true}).click();
    dialog = page.getByRole('dialog',{name:'Resolve address in broader scope',exact:true});
    await dialog.getByRole('button',{name:'Choose allocation context',exact:true}).click();
    await dialog.getByRole('button',{name:'Prepare new allocation',exact:true}).click();
    const allocation = page.getByRole('dialog',{name:'Allocate network resource',exact:true});
    await allocation.getByLabel('Name',{exact:true}).fill('Guest allocation');
    await allocation.getByLabel('Address / prefix',{exact:true}).fill('192.0.2.80/32');
    await allocation.getByLabel('Address space',{exact:true}).selectOption({label:await allocation.getByLabel('Address space',{exact:true}).locator('option').filter({hasText:'Real namespace'}).textContent()});
    await allocation.getByRole('button',{name:'Review allocation',exact:true}).click(); await expect(allocation).toBeHidden();
    await expect(dialog).toContainText('commit together');
    await dialog.getByLabel('Intended assignment',{exact:true}).selectOption(`${iface.asset_id}/${iface.incarnation}`);
    await dialog.getByRole('button',{name:'Apply reviewed mapping',exact:true}).click(); await expect(dialog).toBeHidden();
    await expect(page.getByRole('region',{name:'Resource details'}).locator('dt').filter({hasText:/^binding$/})).toHaveCount(1);
    const hash = new URL(page.url()).hash; await server.stop(); server = await host(dir); await page.goto(server.url+hash);
    await expect(page.getByRole('region',{name:'Resource details'})).toContainText('192.0.2.80');
    await page.goto(server.url); await page.getByRole('button',{name:'Guest allocation',exact:true}).click();
    await expect(page.getByRole('button',{name:`Interface ${iface.asset_id.slice(-8)}`,exact:true})).toBeVisible();
  } finally { await server.stop(); await rm(dir,{recursive:true,force:true}); }
});

test('prefix context allocates and retrieves children without reselecting its namespace', async ({ page }) => {
  // The containing prefix is an explicit allocation context, not an inherited VRF.
  const dir = await mkdtemp(join(tmpdir(), 'omloom-prefix-ui-')); const server = await host(dir);
  try {
    await page.goto(server.url); await create(page,'Namespace under review','ADDRESS_SPACE');
    await page.getByRole('navigation',{name:'Views'}).getByRole('button',{name:'Network',exact:true}).click();
    await page.getByRole('button',{name:'Allocate IP / prefix',exact:true}).click();
    const dialog = page.getByRole('dialog');
    await expect(dialog).toContainText('Within Namespace under review');
    await expect(dialog.getByLabel('Address space',{exact:true})).toHaveCount(0);
    await dialog.getByLabel('Kind',{exact:true}).selectOption({label:'Prefix'});
    await dialog.getByLabel('Name',{exact:true}).fill('Guest subnet');
    await dialog.getByLabel('Address / prefix',{exact:true}).fill('2001:db8:1::/64');
    await dialog.getByRole('button',{name:'Allocate',exact:true}).click(); await expect(dialog).toBeHidden();
    await page.getByRole('button',{name:'Guest subnet',exact:true}).click();
    await page.getByRole('button',{name:'Allocate IP / prefix',exact:true}).click();
    await expect(dialog).toContainText('Within Guest subnet');
    await dialog.getByLabel('Name',{exact:true}).fill('Guest address');
    await dialog.getByLabel('Address / prefix',{exact:true}).fill('2001:db8:1::10/128');
    await dialog.getByRole('button',{name:'Allocate',exact:true}).click(); await expect(dialog).toBeHidden();
    await page.getByLabel('Search inventory').fill('2001:db8:1::10');
    await page.getByRole('button',{name:'Search',exact:true}).click();
    await expect(page.getByRole('button',{name:'Guest address',exact:true})).toBeVisible();
    await page.reload(); await expect(page.getByRole('button',{name:'Guest address',exact:true})).toBeVisible();
  } finally { await server.stop(); await rm(dir,{recursive:true,force:true}); }
});

test('workspace creation starts with the relevant resource type', async ({ page }) => {
  // Views share inventory but should not make every creation start as a physical device.
  const dir = await mkdtemp(join(tmpdir(), 'omloom-context-ui-'));
  const server = await host(dir);
  try {
    await page.goto(server.url);
    for (const [view, kind] of [['Virtualization', 'virtual machine'], ['Services', 'service'], ['Hardware', 'device']]) {
      await page.getByRole('navigation', { name: 'Views' }).getByRole('button', { name: view, exact: true }).click();
      await page.getByRole('button', { name: '+ Create resource', exact: true }).click();
      await expect(page.getByRole('dialog').getByLabel('Resource kind', { exact: true }).locator('option:checked')).toHaveText(kind);
      await page.getByRole('dialog').getByRole('button', { name: 'Cancel', exact: true }).click();
    }
    await page.getByRole('navigation', { name: 'Views' }).getByRole('button', { name: 'Network', exact: true }).click();
    await page.getByRole('button', { name: '+ Create resource', exact: true }).click();
    await expect(page.getByRole('dialog').getByRole('heading', { name: 'Record known address', exact: true })).toBeVisible();
    await page.getByRole('dialog').getByLabel('IP address', { exact: true }).fill('192.0.2.17');
    await expect(page.getByRole('dialog').getByLabel('Address space', { exact: true })).toHaveCount(0);
    await expect(page.getByRole('dialog').getByLabel('Routing context', { exact: true })).toHaveCount(0);
    await page.getByRole('dialog').getByRole('button', { name: 'Save known address', exact: true }).click();
    await expect(page.getByRole('dialog')).toBeHidden();
    await expect(page.getByRole('region', { name: 'Resource details' })).toContainText('192.0.2.17');
    await page.getByRole('button', { name: 'Close resource details', exact: true }).click();
    await page.getByRole('button', { name: 'Allocate IP / prefix', exact: true }).click();
    await expect(page.getByRole('dialog').getByRole('heading', { name: 'Allocate network resource', exact: true })).toBeVisible();
    await page.getByRole('dialog').getByLabel('Name', { exact: true }).fill('Pending address');
    await page.getByRole('dialog').getByRole('button', { name: 'Create address space', exact: true }).click();
    await page.getByRole('dialog', { name: 'Create resource', exact: true }).getByLabel('Name', { exact: true }).fill('Local network scope');
    await page.getByRole('dialog', { name: 'Create resource', exact: true }).getByRole('button', { name: 'Save resource', exact: true }).click();
    await expect(page.getByRole('dialog').getByRole('heading', { name: 'Allocate network resource', exact: true })).toBeVisible();
    await expect(page.getByRole('dialog').getByLabel('Address space', { exact: true }).locator('option').filter({ hasText: 'Local network scope' })).toHaveCount(1);
    await expect(page.getByRole('dialog').getByLabel('Name', { exact: true })).toHaveValue('Pending address');
  } finally { await server.stop(); await rm(dir, { recursive: true, force: true }); }
});

test('dense containers paginate and restore the enclosing view on desktop and phone', async ({page}) => {
  // Fixture creation is separate from the bounded user actions; each UI action retains its normal timeout.
  test.setTimeout(60_000);
  const {buildSync}=await import('esbuild');
  const {GrpcWebFetchTransport}=await import('@protobuf-ts/grpcweb-transport');
  const built=buildSync({stdin:{contents:'export * from "./web/src/generated/omloom/v1/workspace"; export {AssetKind} from "./web/src/generated/omloom/v1/contracts";',resolveDir:process.cwd()},bundle:true,write:false,format:'esm',platform:'node'});
  const m=await import('data:text/javascript;base64,'+Buffer.from(built.outputFiles[0].contents).toString('base64'));
  const dir=await mkdtemp(join(tmpdir(),'omloom-dense-ui-'));const server=await host(dir);
  try {
    const transport=new GrpcWebFetchTransport({baseUrl:server.url,format:'binary'});
    async function rpc(command,focus){
      const q=await transport.unary(m.Workspace.methods[0],m.Request.create({command:{oneofKind:'query',query:m.Query.create({limit:1})}}),transport.mergeOptions());
      return (await transport.unary(m.Workspace.methods[0],m.Request.create({command,working:{roots:[],focus,expected:q.response.result.page.revision}}),transport.mergeOptions())).response.result;
    }
    async function object(kind,name,parent){return (await rpc({oneofKind:'write',write:m.Write.create({kind,parent,changes:[{name:'name',value:{oneofKind:'text',text:name}}]})},parent)).mutation.targets[0];}
    const device=await object(m.AssetKind.DEVICE,'Dense chassis');
    for(let i=0;i<129;i++) await object(m.AssetKind.PORT,`Port ${String(i).padStart(3,'0')}`,device);
    const root=`${server.url}/#/?view=2&search=Dense&sort=name&desc=1&columns=name,serial`;
    for(const width of [1440,390]){
      await page.setViewportSize({width,height:844});await page.goto(root);
      await page.getByRole('checkbox',{name:'Select Dense chassis',exact:true}).check();
      await page.getByRole('button',{name:'Dense chassis',exact:true}).click();
      const details=page.getByRole('region',{name:'Resource details',exact:true});
      await expect(details.getByRole('heading',{name:'Dense chassis',exact:true})).toBeVisible();
      await expect(page.getByRole('alert')).toHaveCount(0);
      await expect(details.getByRole('button',{name:'Next relationships',exact:true})).toBeVisible();
      const firstRelations=await details.locator(':scope > ul').innerText();
      await details.getByRole('button',{name:'Next relationships',exact:true}).click();
      await expect.poll(()=>details.locator(':scope > ul').innerText()).not.toBe(firstRelations);
      if(width<700) await page.getByLabel('Resource perspective',{exact:true}).selectOption('4');
      else await page.getByRole('navigation',{name:'Views',exact:true}).getByRole('button',{name:'Services',exact:true}).click();
      await details.getByRole('button',{name:'Close resource details',exact:true}).click();
      await expect(page.getByRole('heading',{name:'Hardware',exact:true})).toBeVisible();
      await expect(page.getByLabel('Search inventory',{exact:true})).toHaveValue('Dense');
      await expect(page.getByRole('checkbox',{name:'Select Dense chassis',exact:true})).toBeChecked();
      await page.getByRole('button',{name:'Dense chassis',exact:true}).click();
      await page.reload();
      await details.getByRole('button',{name:'Close resource details',exact:true}).click();
      await expect(page.getByRole('heading',{name:'Hardware',exact:true})).toBeVisible();
      await expect(page.getByLabel('Search inventory',{exact:true})).toHaveValue('Dense');
      expect(new URL(page.url()).hash).toContain('desc=1');
      expect(new URL(page.url()).hash).toContain('columns=name,serial');
      expect(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth)).toBe(true);
      await page.screenshot({path:`test-results/dense-container-${width}.png`,fullPage:true});
    }
  } finally {await server.stop();await rm(dir,{recursive:true,force:true});}
});

test('phone inventory opens a dedicated detail and returns to the same search', async ({ page }) => {
  // A phone should not require scrolling past the desktop table to inspect or edit its selected asset.
  const dir = await mkdtemp(join(tmpdir(), 'omloom-phone-ui-'));
  const server = await host(dir);
  try {
    await page.goto(server.url);
    await create(page, 'Field router');
    for (const name of ['Field node A', 'Field node B', 'Field node C', 'Field node D']) await create(page, name);
    await page.getByRole('button', { name: 'Close resource details', exact: true }).click();
    await page.setViewportSize({ width: 390, height: 844 });
    await expect(page.getByRole('button', { name: 'Import CSV', exact: true })).toBeHidden();
    await page.getByRole('button', { name: 'More', exact: true }).click();
    await expect(page.getByRole('button', { name: 'Import CSV', exact: true })).toBeVisible();
    await expect(page.getByRole('button', { name: 'Refresh inventory', exact: true })).toBeVisible();
    await page.keyboard.press('Escape');
    await expect(page.getByRole('button', { name: 'More', exact: true })).toHaveAttribute('aria-expanded', 'false');
    expect((await page.locator('tbody tr').first().boundingBox()).y).toBeLessThan(560);
    await page.getByLabel('Search inventory').fill('Field');
    await page.getByRole('button', { name: 'Search', exact: true }).click();
    await page.getByRole('checkbox', { name: 'Select Field router', exact: true }).check();
    await page.getByRole('button', { name: 'Field router', exact: true }).scrollIntoViewIfNeeded();
    const listPosition = await page.evaluate(() => scrollY);
    expect(listPosition).toBeGreaterThan(0);
    await page.getByRole('button', { name: 'Field router', exact: true }).click();
    await expect(page.getByRole('region', { name: 'Inventory', exact: true })).toBeHidden();
    await expect(page.getByRole('region', { name: 'Resource details' })).toBeVisible();
    await page.screenshot({ path: 'test-results/phone-resource-detail.png', fullPage: true });
    await page.getByRole('button', { name: 'Edit resource', exact: true }).click();
    const dialog = page.getByRole('dialog');
    await dialog.getByLabel('Description change', { exact: true }).selectOption('set');
    await dialog.getByLabel('Description', { exact: true }).fill('Checked on site');
    await page.screenshot({ path: 'test-results/phone-edit-resource.png', fullPage: true });
    await dialog.getByRole('button', { name: 'Save resource', exact: true }).click();
    await expect(dialog).toBeHidden();
    await expect(page.getByRole('region', { name: 'Resource details' })).toContainText('Checked on site');
    // Changing representation inside a detail must not replace its return destination.
    await page.getByLabel('Resource perspective', { exact: true }).selectOption('4');
    await page.getByRole('button', { name: 'Back to inventory', exact: true }).click();
    await expect(page.getByRole('heading', { name: 'Overview', exact: true })).toBeVisible();
    await expect(page.getByLabel('Search inventory')).toHaveValue('Field');
    await expect.poll(() => page.evaluate(() => scrollY)).toBe(listPosition);
    await expect(page.getByRole('checkbox', { name: 'Select Field router', exact: true })).toBeChecked();
    await expect(page.getByRole('button', { name: 'Field router', exact: true })).toBeVisible();
    await page.getByLabel('Sort resources', { exact: true }).selectOption('name:desc');
    await page.getByLabel('Select phone page', { exact: true }).check();
    await expect(page.getByText('5 selected on this page', { exact: false })).toBeVisible();
    await page.getByRole('button', { name: 'Clear selection', exact: true }).click();
    await page.screenshot({ path: 'test-results/phone-inventory-list.png', fullPage: true });
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    await page.reload();
    await expect(page.getByLabel('Search inventory')).toHaveValue('Field');
  } finally { await server.stop(); await rm(dir, { recursive: true, force: true }); }
});

test('shared gateway addresses use one durable group and intended participant set', async ({ page }) => {
  // One group spans two IPs; the UI must send its revision when editing through either address.
  const dir = await mkdtemp(join(tmpdir(), 'omloom-ipam-ui-'));
  let server = await host(dir);
  try {
    await page.goto(server.url);
    await create(page, 'Tenant addresses', 'ADDRESS_SPACE');
    await create(page, 'Edge LAN', 'L2_DOMAIN');
    await create(page, 'Router uplink', 'INTERFACE');
    await page.getByRole('button', { name: 'Close resource details', exact: true }).click();
    await page.getByRole('navigation', { name: 'Views' }).getByRole('button', { name: 'Network', exact: false }).click();
    for (const [index, name] of ['Gateway A', 'Gateway B'].entries()) {
      await page.getByRole('button', { name: 'Allocate IP / prefix', exact: true }).click();
      const dialog = page.getByRole('dialog');
      await dialog.getByLabel('Name', { exact: true }).fill(name);
      await dialog.getByLabel('Address / prefix', { exact: true }).fill(`192.0.2.${index + 1}/24`);
      await dialog.getByLabel('Address space', { exact: true }).selectOption({ label: await dialog.getByLabel('Address space', { exact: true }).locator('option').filter({ hasText: 'Tenant addresses' }).textContent() });
      await dialog.getByLabel('Address use', { exact: true }).selectOption({ label: 'Explicit VRRP group' });
      if (index === 0) {
        await dialog.getByLabel('L2 domain', { exact: true }).selectOption({ label: await dialog.getByLabel('L2 domain', { exact: true }).locator('option').filter({ hasText: 'Edge LAN' }).textContent() });
        await dialog.getByLabel('New group name', { exact: true }).fill('Edge routers');
        await dialog.getByLabel('VRID', { exact: true }).fill('7');
      } else {
        await dialog.getByLabel('Group selection', { exact: true }).selectOption('existing');
        await dialog.getByLabel('Shared address group', { exact: true }).selectOption({ label: await dialog.getByLabel('Shared address group', { exact: true }).locator('option').filter({ hasText: 'Edge routers' }).textContent() });
      }
      await dialog.getByRole('button', { name: 'Allocate', exact: true }).click();
      await expect(dialog).toBeHidden();
    }
    await page.getByRole('button', { name: 'Gateway A', exact: true }).click();
    await page.getByRole('button', { name: 'Assign participant', exact: true }).click();
    const dialog = page.getByRole('dialog');
    await dialog.getByLabel('Interface', { exact: true }).selectOption({ label: await dialog.getByLabel('Interface', { exact: true }).locator('option').filter({ hasText: 'Router uplink' }).textContent() });
    await dialog.getByLabel('Intended priority', { exact: true }).fill('255');
    await dialog.getByRole('button', { name: 'Assign participant', exact: true }).click();
    await expect(dialog).toBeHidden();
    await page.getByRole('button', { name: 'Close resource details', exact: true }).click();
    await page.getByRole('button', { name: 'Gateway B', exact: true }).click();
    await expect(page.getByRole('region', { name: 'Resource details' })).toContainText('Intended priority 255');
    const hash = new URL(page.url()).hash;
    await server.stop(); server = await host(dir);
    await page.goto(server.url + hash);
    await expect(page.getByRole('region', { name: 'Resource details' })).toContainText('Intended priority 255');
    await page.getByRole('button', { name: 'Remove participant', exact: true }).click();
    await expect(page.getByRole('button', { name: 'Remove participant', exact: true })).toHaveCount(0);
    await page.getByRole('button', { name: 'Close resource details', exact: true }).click();
    await page.getByRole('button', { name: 'Gateway A', exact: true }).click();
    await expect(page.getByRole('region', { name: 'Resource details' })).toContainText('Shared group:');
    await expect(page.getByRole('button', { name: 'Remove participant', exact: true })).toHaveCount(0);
    await page.screenshot({ path: 'test-results/shared-address-group.png', fullPage: true });
  } finally { await server.stop(); await rm(dir, { recursive: true, force: true }); }
});

test('team publication previews its audience and saved adjustments reset without granting data', async ({ page }) => {
  // The publication preview has no durable effect until confirmed; a different fixture sees only its own data scope.
  const dir = await mkdtemp(join(tmpdir(), 'omloom-view-ui-'));
  let server = await host(dir);
  try {
    await page.goto(server.url);
    await create(page, 'Private hypervisor');
    await create(page, 'Team VM', 'VIRTUAL_MACHINE');
    await page.getByRole('button', { name: 'Close resource details', exact: true }).click();
    await page.getByLabel('Search inventory').fill('Private');
    await page.getByRole('button', { name: 'Search', exact: true }).click();
    await page.getByRole('button', { name: 'Save view', exact: true }).click();
    const dialog = page.getByRole('dialog');
    await dialog.getByLabel('Name', { exact: true }).fill('Hypervisor selection');
    await dialog.getByRole('checkbox', { name: 'operations', exact: true }).check();
    await dialog.getByRole('checkbox', { name: 'operations may edit this definition', exact: true }).check();
    await dialog.getByRole('button', { name: 'Preview publication', exact: true }).click();
    await expect(dialog).toContainText('Publish the name, selected roots, predicates and columns to operations');
    await expect(page.getByRole('button', { name: '♧ Hypervisor selection', exact: true })).toHaveCount(0);
    await dialog.getByRole('button', { name: 'Publish reviewed definition', exact: true }).click();
    await expect(dialog).toBeHidden();
    await page.getByRole('button', { name: '♧ Hypervisor selection', exact: true }).click();
    await page.getByLabel('Search inventory').fill('Team');
    await page.getByRole('button', { name: 'Search', exact: true }).click();
    await expect(page.getByRole('button', { name: 'Team VM', exact: true })).toBeVisible();
    await page.getByRole('button', { name: 'Reset adjustments', exact: true }).click();
    await expect(page.getByLabel('Search inventory')).toHaveValue('Private');
    await page.getByLabel('Find saved views').fill('missing');
    await expect(page.getByRole('button', { name: '♧ Hypervisor selection', exact: true })).toHaveCount(0);
    await page.getByLabel('Find saved views').fill('Hypervisor');
    await expect(page.getByRole('button', { name: '♧ Hypervisor selection', exact: true })).toBeVisible();
    const limited = join(dir, 'view-reader.json');
    await writeFile(limited, JSON.stringify([{ selection: { scope: 'kinds', kinds: ['ASSET_KIND_VIRTUAL_MACHINE'] }, read: { mode: 'all' }, write: { mode: 'all' }, writable: true }]));
    await server.stop(); server = await host(dir, 'devops', limited);
    await page.goto(server.url);
    await page.getByRole('button', { name: '♧ Hypervisor selection', exact: true }).click();
    await expect(page.getByRole('button', { name: 'Private hypervisor', exact: true })).toHaveCount(0);
    await expect(page.getByRole('region', { name: 'Inventory' })).toContainText('0 resources');
  } finally { await server.stop(); await rm(dir, { recursive: true, force: true }); }
});

test('real disk inventory supports editing, context, private saves and restart', async ({ page }) => {
  // Exercise the actual UI and grpc-web host, then reopen the same database.
  const dir = await mkdtemp(join(tmpdir(), 'omloom-ui-'));
  let server = await host(dir);
  try {
    await page.goto(server.url);
    await expect(page.getByRole('heading', { name: 'Start with the infrastructure you know' })).toBeVisible();
    await create(page, 'Edge router');
    await create(page, 'Build worker', 'VIRTUAL_MACHINE');
    await page.getByRole('button', { name: 'Close resource details', exact: true }).click();
    await page.getByRole('button', { name: 'Edge router', exact: true }).click();
    await expect(page.getByRole('region', { name: 'Resource details' })).toContainText('Stable identity');
    const originalUrl = page.url();
    await page.getByRole('button', { name: 'Edit resource', exact: true }).click();
    const dialog = page.getByRole('dialog');
    await dialog.getByLabel('Name change', { exact: true }).selectOption('set');
    await dialog.getByLabel('Name', { exact: true }).fill('Edge primary');
    await dialog.getByRole('button', { name: 'Save resource', exact: true }).click();
    await expect(dialog).toBeHidden();
    await expect(page.getByRole('region', { name: 'Resource details' })).toContainText('Edge primary');
    await page.reload();
    await expect(page.getByRole('region', { name: 'Resource details' })).toContainText('Edge primary');
    await page.getByLabel('Search inventory').fill('Edge');
    await page.getByRole('button', { name: 'Search', exact: true }).click();
    await expect(page.getByRole('button', { name: 'Build worker', exact: true })).toHaveCount(0);
    await page.getByRole('button', { name: 'Save view', exact: true }).click();
    await page.getByRole('dialog').getByLabel('Name', { exact: true }).fill('Network ownership');
    await page.getByRole('button', { name: 'Save personal view', exact: true }).click();
    await expect(page.getByRole('dialog')).toBeHidden();
    await expect(page.getByRole('button', { name: '◦ Network ownership', exact: true })).toBeVisible();
    await server.stop(); server = await host(dir);
    await page.goto(server.url + new URL(originalUrl).hash);
    await expect(page.getByRole('region', { name: 'Resource details' })).toContainText('Edge primary');
    await expect(page.getByRole('button', { name: '◦ Network ownership', exact: true })).toBeVisible();
    await page.setViewportSize({ width: 390, height: 844 });
    await expect(page.getByRole('heading', { name: 'Overview', exact: true })).toBeVisible();
    await page.screenshot({ path: 'test-results/inventory-mobile.png', fullPage: true });
  } finally { await server.stop(); await rm(dir, { recursive: true, force: true }); }
});

test('departments share one backup host while namespace and host projections remain separate', async ({ page }) => {
  // Grant files are generated from actual returned IDs and selected by the host,
  // never by browser headers, query parameters or synthetic copies of a host.
  const dir = await mkdtemp(join(tmpdir(), 'omloom-departments-'));
  let server = await host(join(dir, 'db'));
  try {
    await page.goto(server.url);
    const backup = await create(page, 'Shared backup host');
    const devops = await create(page, 'DevOps hypervisor');
    const engineer = await create(page, 'Service engineering host');
    const vm = await create(page, 'Development VM', 'VIRTUAL_MACHINE');
    const support = await create(page, 'Service VM', 'VIRTUAL_MACHINE');
    const devopsStorage = await create(page, 'Operations backup namespace', 'STORAGE');
    const serviceStorage = await create(page, 'Engineering backup namespace', 'STORAGE');
    for (const storage of [devopsStorage, serviceStorage]) {
      await page.goto(`${server.url}/#/?asset=${storage.asset_id}&incarnation=${storage.incarnation}`);
      await page.getByRole('button', { name: 'Connections in broader scope', exact: true }).click();
      const dialog = page.getByRole('dialog');
      await dialog.getByRole('button', { name: 'Choose connection endpoints', exact: true }).click();
      await dialog.getByRole('combobox', { name: 'Other endpoint', exact: true }).selectOption(`${backup.asset_id}/${backup.incarnation}`);
      await dialog.getByRole('button', { name: 'Review connection', exact: true }).click();
      await dialog.getByRole('combobox', { name: 'Relationship', exact: true }).selectOption({ label: 'on' }, { timeout: 5000 });
      await dialog.getByRole('button', { name: 'Add relationship', exact: true }).click();
      await expect(dialog).toBeHidden();
      await page.getByRole('button', { name: 'Connections in broader scope', exact: true }).click();
      await dialog.getByRole('button', { name: 'Review existing connections', exact: true }).click();
      await expect(dialog).toContainText('Shared backup host');
      await dialog.getByRole('button', { name: 'Return to resource', exact: true }).click();
    }
    const grant = targets => ({ selection: { scope: 'targets', targets }, read: { mode: 'all' }, write: { mode: 'all' }, writable: true });
    const operationsFile = join(dir, 'operations.json');
    const engineeringFile = join(dir, 'engineering.json');
    const developerFile = join(dir, 'developer.json');
    await writeFile(operationsFile, JSON.stringify([grant([backup, devops, vm, devopsStorage])]));
    await writeFile(engineeringFile, JSON.stringify([grant([backup, engineer, support, serviceStorage])]));
    await writeFile(developerFile, JSON.stringify([grant([vm])]));
    await server.stop(); server = await host(join(dir, 'db'), 'devops', operationsFile);
    await page.goto(server.url);
    await expect(page.getByRole('button', { name: 'Shared backup host', exact: true })).toHaveCount(1);
    await expect(page.getByRole('button', { name: 'Operations backup namespace', exact: true })).toHaveCount(0);
    await expect(page.getByRole('button', { name: 'Engineering backup namespace', exact: true })).toHaveCount(0);
    await expect(page.getByRole('button', { name: 'Service engineering host', exact: true })).toHaveCount(0);
    await page.getByRole('button', { name: 'Shared backup host', exact: true }).click();
    expect(new URLSearchParams(new URL(page.url()).hash.split('?')[1]).get('asset')).toBe(backup.asset_id);
    await expect(page.getByRole('button', { name: 'on · Operations backup namespace', exact: true })).toBeVisible();
    await expect(page.getByRole('region', { name: 'Resource details' })).not.toContainText('Engineering backup namespace');
    await server.stop(); server = await host(join(dir, 'db'), 'engineer', engineeringFile);
    await page.goto(server.url);
    await expect(page.getByRole('button', { name: 'Shared backup host', exact: true })).toHaveCount(1);
    await expect(page.getByRole('button', { name: 'Engineering backup namespace', exact: true })).toHaveCount(0);
    await expect(page.getByRole('button', { name: 'Operations backup namespace', exact: true })).toHaveCount(0);
    await expect(page.getByRole('button', { name: 'DevOps hypervisor', exact: true })).toHaveCount(0);
    await page.getByRole('button', { name: 'Shared backup host', exact: true }).click();
    await expect(page.getByRole('button', { name: 'on · Engineering backup namespace', exact: true })).toBeVisible();
    await expect(page.getByRole('region', { name: 'Resource details' })).not.toContainText('Operations backup namespace');
    await server.stop(); server = await host(join(dir, 'db'), 'developer', developerFile);
    await page.goto(server.url);
    await expect(page.getByRole('button', { name: 'Development VM', exact: true })).toBeVisible();
    await expect(page.getByRole('button', { name: 'Shared backup host', exact: true })).toHaveCount(0);
    await page.goto(`${server.url}/#/?kind=3&actor=fixture-operator`);
    await expect(page.getByRole('button', { name: 'Shared backup host', exact: true })).toHaveCount(0);
  } finally { await server.stop(); await rm(dir, { recursive: true, force: true }); }
});

test('CSV preview validates before acceptance and scope is selected by the server', async ({ page }) => {
  // No browser login mock or API interception: both fixture hosts open real disk state.
  const dir = await mkdtemp(join(tmpdir(), 'omloom-import-ui-'));
  let server = await host(dir);
  try {
    await page.goto(server.url);
    await expect(page.getByRole('heading', { name: 'Overview', exact: true })).toBeVisible();
    await page.getByRole('button', { name: 'Import CSV', exact: true }).click();
    const dialog = page.getByRole('dialog');
    await dialog.getByLabel('Source name').fill('rack-register');
    await dialog.getByLabel('CSV', { exact: true }).fill('external_id,name\nrouter-1,Imported edge');
    await dialog.getByRole('button', { name: 'Prepare preview' }).click();
    await expect(dialog).toContainText('Create new identity');
    await expect(page.getByRole('button', { name: 'Imported edge', exact: true })).toHaveCount(0);
    await dialog.getByRole('button', { name: 'Accept reviewed rows' }).click();
    await expect(dialog).toBeHidden();
    await expect(page.getByRole('button', { name: 'Imported edge', exact: true })).toBeVisible();
    await create(page, 'App VM', 'VIRTUAL_MACHINE');
    await server.stop(); server = await host(dir, 'developer');
    await page.goto(server.url);
    await expect(page.getByRole('button', { name: 'App VM', exact: true })).toBeVisible();
    await expect(page.getByRole('button', { name: 'Imported edge', exact: true })).toHaveCount(0);
    await expect(page.getByRole('button', { name: 'Company fields', exact: true })).toHaveCount(0);
  } finally { await server.stop(); await rm(dir, { recursive: true, force: true }); }
});

test('selected roots survive view changes, personal save and reload', async ({ page }) => {
  // Saving a set of roots must not discard every root after the first or broaden scope.
  const dir = await mkdtemp(join(tmpdir(), 'omloom-root-set-'));
  const server = await host(dir);
  try {
    await page.goto(server.url);
    const firstRoot = await create(page, 'First scope host');
    const secondRoot = await create(page, 'Second scope VM', 'VIRTUAL_MACHINE');
    await create(page, 'Outside selection');
    await page.getByRole('button', { name: 'Close resource details', exact: true }).click();
    await page.getByRole('checkbox', { name: 'Select First scope host', exact: true }).check();
    await page.getByRole('checkbox', { name: 'Select Second scope VM', exact: true }).check();
    await page.getByRole('button', { name: 'Use selected as scope', exact: true }).click({ timeout: 5000 });
    await expect(page.getByRole('button', { name: 'Outside selection', exact: true })).toHaveCount(0);
    await page.getByRole('navigation', { name: 'Views' }).getByRole('button', { name: 'Services', exact: true }).click();
    await expect(page.getByRole('heading',{name:'No resources at this level in this view',exact:true})).toBeVisible();
    await expect(page.getByRole('button',{name:'2 selected roots',exact:true})).toBeVisible();
    await page.getByRole('button', { name: 'Save view', exact: true }).click();
    await page.getByRole('dialog').getByLabel('Name', { exact: true }).fill('Two selected roots');
    await page.getByRole('button', { name: 'Save personal view', exact: true }).click();
    await expect(page.getByRole('dialog')).toBeHidden();
    await page.goto(server.url);
    await expect(page.getByRole('button', { name: 'Outside selection', exact: true })).toBeVisible();
    await page.getByRole('button', { name: '◦ Two selected roots', exact: true }).click();
    await page.reload();
    await expect(page.getByRole('heading',{name:'Services',exact:true})).toBeVisible();
    await expect(page.getByRole('heading',{name:'No resources at this level in this view',exact:true})).toBeVisible();
    await expect(page.getByRole('button',{name:'2 selected roots',exact:true})).toBeVisible();
    await page.getByRole('navigation',{name:'Views'}).getByRole('button',{name:'Overview',exact:true}).click();
    // Selected roots define the enclosing level; they are not their own children.
    await expect(page.getByRole('heading',{name:'No resources at this level in this view',exact:true})).toBeVisible();
    const restored = new URLSearchParams(new URL(page.url()).hash.split('?')[1]);
    expect(restored.getAll('scope').sort()).toEqual([firstRoot.asset_id,secondRoot.asset_id].sort());
    expect(restored.getAll('scopeInc').sort()).toEqual([firstRoot.incarnation,secondRoot.incarnation].sort());
    await expect(page.getByRole('button', { name: 'First scope host', exact: true })).toHaveCount(0);
    await expect(page.getByRole('button', { name: 'Second scope VM', exact: true })).toHaveCount(0);
    await expect(page.getByRole('button', { name: 'Outside selection', exact: true })).toHaveCount(0);
  } finally { await server.stop(); await rm(dir, { recursive: true, force: true }); }
});
