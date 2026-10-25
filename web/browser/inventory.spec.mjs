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
