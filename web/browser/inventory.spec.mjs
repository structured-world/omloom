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
  await page.getByRole('button', { name: '+ Create resource', exact: true }).click();
  const dialog = page.getByRole('dialog');
  await dialog.getByLabel('Resource kind', { exact: true }).selectOption({ label: kind.toLowerCase().replaceAll('_', ' ') }, { timeout: 5000 });
  await dialog.getByLabel('Name', { exact: true }).fill(name, { timeout: 5000 });
  await dialog.getByRole('button', { name: 'Save resource', exact: true }).click();
  await expect(dialog).toBeHidden();
  await expect(page.getByRole('button', { name, exact: true })).toBeVisible();
  await page.getByRole('button', { name, exact: true }).click();
  await expect(page.getByRole('region', { name: 'Resource details' })).toBeVisible();
  const hash = new URL(page.url()).hash;
  const params = new URLSearchParams(hash.slice(hash.indexOf('?') + 1));
  return { asset_id: params.get('asset'), incarnation: params.get('incarnation') };
}

test('shared gateway addresses use one durable group and intended participant set', async ({ page }) => {
  // One group spans two IPs; the UI must send its revision when editing through either address.
  const dir = await mkdtemp(join(tmpdir(), 'omloom-ipam-ui-'));
  let server = await host(dir);
  try {
    await page.goto(server.url);
    await create(page, 'Tenant addresses', 'ADDRESS_SPACE');
    await create(page, 'Edge LAN', 'L2_DOMAIN');
    await create(page, 'Router uplink', 'INTERFACE');
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
    await page.getByRole('button', { name: 'Gateway B', exact: true }).click();
    await expect(page.getByRole('region', { name: 'Resource details' })).toContainText('Intended priority 255');
    const hash = new URL(page.url()).hash;
    await server.stop(); server = await host(dir);
    await page.goto(server.url + hash);
    await expect(page.getByRole('region', { name: 'Resource details' })).toContainText('Intended priority 255');
    await page.getByRole('button', { name: 'Remove participant', exact: true }).click();
    await expect(page.getByRole('button', { name: 'Remove participant', exact: true })).toHaveCount(0);
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
      await page.getByRole('button', { name: 'Add relationship', exact: true }).click();
      const dialog = page.getByRole('dialog');
      await dialog.getByRole('combobox', { name: 'Relationship', exact: true }).selectOption({ label: 'on' }, { timeout: 5000 });
      await dialog.getByRole('combobox', { name: 'Destination', exact: true }).selectOption(`${backup.asset_id}/${backup.incarnation}`);
      await dialog.getByRole('button', { name: 'Add relationship', exact: true }).click();
      await expect(dialog).toBeHidden();
      await expect(page.getByRole('region', { name: 'Resource details' })).toContainText(backup.asset_id.slice(-8));
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
    await expect(page.getByRole('button', { name: 'Operations backup namespace', exact: true })).toBeVisible();
    await expect(page.getByRole('button', { name: 'Engineering backup namespace', exact: true })).toHaveCount(0);
    await expect(page.getByRole('button', { name: 'Service engineering host', exact: true })).toHaveCount(0);
    await page.getByRole('button', { name: 'Shared backup host', exact: true }).click();
    expect(new URLSearchParams(new URL(page.url()).hash.split('?')[1]).get('asset')).toBe(backup.asset_id);
    await server.stop(); server = await host(join(dir, 'db'), 'engineer', engineeringFile);
    await page.goto(server.url);
    await expect(page.getByRole('button', { name: 'Shared backup host', exact: true })).toHaveCount(1);
    await expect(page.getByRole('button', { name: 'Engineering backup namespace', exact: true })).toBeVisible();
    await expect(page.getByRole('button', { name: 'Operations backup namespace', exact: true })).toHaveCount(0);
    await expect(page.getByRole('button', { name: 'DevOps hypervisor', exact: true })).toHaveCount(0);
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
    await create(page, 'First scope host');
    await create(page, 'Second scope VM', 'VIRTUAL_MACHINE');
    await create(page, 'Outside selection');
    await page.getByRole('checkbox', { name: 'Select First scope host', exact: true }).check();
    await page.getByRole('checkbox', { name: 'Select Second scope VM', exact: true }).check();
    await page.getByRole('button', { name: 'Use selected as scope', exact: true }).click({ timeout: 5000 });
    await expect(page.getByRole('button', { name: 'Outside selection', exact: true })).toHaveCount(0);
    await page.getByRole('navigation', { name: 'Views' }).getByRole('button', { name: 'Services', exact: true }).click();
    await expect(page.getByRole('button', { name: 'First scope host', exact: true })).toBeVisible();
    await expect(page.getByRole('button', { name: 'Second scope VM', exact: true })).toBeVisible();
    await page.getByRole('button', { name: 'Save view', exact: true }).click();
    await page.getByRole('dialog').getByLabel('Name', { exact: true }).fill('Two selected roots');
    await page.getByRole('button', { name: 'Save personal view', exact: true }).click();
    await expect(page.getByRole('dialog')).toBeHidden();
    await page.goto(server.url);
    await expect(page.getByRole('button', { name: 'Outside selection', exact: true })).toBeVisible();
    await page.getByRole('button', { name: '◦ Two selected roots', exact: true }).click();
    await page.reload();
    await expect(page.getByRole('button', { name: 'First scope host', exact: true })).toBeVisible();
    await expect(page.getByRole('button', { name: 'Second scope VM', exact: true })).toBeVisible();
    await expect(page.getByRole('button', { name: 'Outside selection', exact: true })).toHaveCount(0);
  } finally { await server.stop(); await rm(dir, { recursive: true, force: true }); }
});
