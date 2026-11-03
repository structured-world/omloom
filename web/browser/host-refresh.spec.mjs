import { test, expect } from '@playwright/test';
import { withFieldHost } from './field-host.mjs';
import { workspaceClient } from './workspace-client.mjs';

for (const width of [1440, 390]) test(`existing host package refresh and uncertain acceptance at ${width}px`, async ({ page }) => {
  // Use the real disk-backed host. A lost HTTP reply must not cause a second
  // import, and selecting a child must not refresh its unselected ancestors.
  await withFieldHost(async url => {
    const { m, rpc } = await workspaceClient(url);
    const field = value => ({ name: 'name', value: { oneofKind: 'text', text: value } });
    const report = m.HostReport.create({ version: 1, platform: 'linux', sourceBasis: 'a'.repeat(64), bootBasis: 'b'.repeat(64), observedAt: 100n, findings: [
      { key: 'hardware', kind: m.AssetKind.DEVICE, fields: [field('Refresh host')] },
      { key: 'installation', kind: m.AssetKind.SOFTWARE_INSTALLATION, parent: 'hardware', fields: [field('Refresh OS')] },
      { key: 'interface/eth0', kind: m.AssetKind.INTERFACE, parent: 'installation', fields: [field('eth0')], addresses: [{ address: '192.0.2.10', prefixLength: 24 }], addressesComplete: true },
    ] });
    const prepared = await rpc({ oneofKind: 'hostImport', hostImport: m.HostImport.create({ source: 'Known source', report, selection: report.findings.map(v => ({ key: v.key })) }) });
    const accepted = await rpc({ oneofKind: 'hostImport', hostImport: m.HostImport.create({ preview: prepared.preview.id }) });
    const [host, os, iface] = accepted.mutation.targets;
    report.observedAt = 200n;
    report.findings[2].addresses = [m.CurrentAddress.create({ address: '192.0.2.20', prefixLength: 24 })];
    report.findings[2].fields = [field('Observed eth0')];
    report.findings.push(m.HostFinding.create({ key: 'interface/new', kind: m.AssetKind.INTERFACE, parent: 'installation', fields: [field('New finding')] }));
    const buffer = Buffer.from(m.Request.toBinary(m.Request.create({ command: { oneofKind: 'hostImport', hostImport: m.HostImport.create({ report }) } })));
    await page.setViewportSize({ width, height: 900 });
    await page.goto(`${url}/#/?asset=${host.assetId}&incarnation=${host.incarnation}`);
    await page.getByRole('button', { name: 'Load observation package', exact: true }).click();
    const dialog = page.getByRole('dialog', { name: 'Refresh host observations', exact: true });
    await dialog.getByLabel('Host report file').setInputFiles({ name: 'refresh.omloom', mimeType: 'application/octet-stream', buffer });
    await expect(dialog.getByRole('combobox', { name: 'Existing source', exact: true })).toHaveValue(/host-mapping\//);
    await expect(dialog.getByRole('checkbox', { name: 'interface: New finding', exact: true })).toBeDisabled();
    await dialog.getByRole('checkbox', { name: 'device: Refresh host', exact: true }).uncheck();
    await dialog.getByRole('checkbox', { name: 'software installation: Refresh OS', exact: true }).uncheck();
    await dialog.getByRole('checkbox', { name: 'This package describes this host and the selected OS installation', exact: true }).check();
    await dialog.getByRole('button', { name: 'Prepare host preview', exact: true }).click();
    await expect(dialog.getByText('Review 1 selected resources', { exact: true })).toBeVisible();
    await expect(dialog.getByText('Add 192.0.2.20/24 (unspecified · unspecified)', { exact: true })).toBeVisible();
    await dialog.getByRole('button', { name: 'Back', exact: true }).click();
    await expect(dialog.getByRole('checkbox', { name: 'interface: Observed eth0', exact: true })).toBeChecked();
    await expect(dialog.getByRole('checkbox', { name: 'device: Refresh host', exact: true })).not.toBeChecked();
    await dialog.getByRole('button', { name: 'Prepare host preview', exact: true }).click();
    await expect(dialog.getByText('Review 1 selected resources', { exact: true })).toBeVisible();
    await rpc({ oneofKind: 'write', write: m.Write.create({ target: host, expected: 1n, kind: m.AssetKind.DEVICE, changes: [{ name: 'description', value: { oneofKind: 'text', text: 'Concurrent manual edit' } }] }) });
    await dialog.getByRole('button', { name: 'Accept selected observations', exact: true }).click();
    await expect(dialog.getByRole('alert')).toContainText('Your selections are retained');
    await expect(dialog.getByRole('checkbox', { name: 'interface: Observed eth0', exact: true })).toBeChecked();
    await dialog.getByRole('button', { name: 'Prepare host preview', exact: true }).click();
    let lost = false;
    await page.route('**/omloom.workspace.v1.Workspace/Call', async route => {
      const request = m.Request.fromBinary(route.request().postDataBuffer().subarray(5));
      if (!lost && request.command.oneofKind === 'hostImport' && request.command.hostImport.preview) {
        lost = true; await route.fetch(); await route.abort('failed');
      } else await route.continue();
    });
    await dialog.getByRole('button', { name: 'Accept selected observations', exact: true }).click();
    await expect(dialog.getByRole('alert')).toContainText('Outcome unknown');
    await expect(dialog.getByRole('button', { name: 'Back', exact: true })).toBeDisabled();
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    await dialog.getByRole('button', { name: 'Retry same acceptance', exact: true }).click();
    await expect(dialog).toBeHidden();
    const detail = (await rpc({ oneofKind: 'read', read: m.Read.create({ target: iface }) })).detail;
    expect(detail.item.fields).toEqual([field('eth0')]);
    expect(detail.observed[0].evidence.fields).toEqual([field('Observed eth0')]);
    expect(detail.currentAddresses[0].observations.map(v => v.value.address)).toEqual(['192.0.2.20']);
    expect((await rpc({ oneofKind: 'read', read: m.Read.create({ target: os }) })).detail.observed).toEqual([]);
    expect((await rpc({ oneofKind: 'query', query: m.Query.create({ search: 'New finding', limit: 25 }) })).page.total).toBe(0n);
    await page.unrouteAll();
  });
});

test('late source lookup cannot replace the newly selected package', async ({ page }) => {
  // Both lookups reach the real host. Only delivery is delayed, so an older
  // empty response cannot clear the current source selection or enable adoption.
  await withFieldHost(async url => {
    const { m, rpc } = await workspaceClient(url);
    const report = m.HostReport.create({ version: 1, platform: 'linux', sourceBasis: 'a'.repeat(64), bootBasis: 'b'.repeat(64), observedAt: 100n, findings: [{ key: 'hardware', kind: m.AssetKind.DEVICE, fields: [{ name: 'name', value: { oneofKind: 'text', text: 'Late source host' } }] }] });
    const review = await rpc({ oneofKind: 'hostImport', hostImport: m.HostImport.create({ source: 'Retained source', report, selection: [{ key: 'hardware' }] }) });
    const host = (await rpc({ oneofKind: 'hostImport', hostImport: m.HostImport.create({ preview: review.preview.id }) })).mutation.targets[0];
    await page.goto(`${url}/#/?asset=${host.assetId}&incarnation=${host.incarnation}`);
    await page.getByRole('button', { name: 'Load observation package', exact: true }).click();
    let release, started, delivered;
    const hold = new Promise(resolve => { release = resolve; });
    const oldStarted = new Promise(resolve => { started = resolve; });
    const oldDelivered = new Promise(resolve => { delivered = resolve; });
    let first = true;
    await page.route('**/omloom.workspace.v1.Workspace/Call', async route => {
      const request = m.Request.fromBinary(route.request().postDataBuffer().subarray(5));
      if (first && request.command.oneofKind === 'hostSources') {
        first = false;
        const response = await route.fetch(); started(); await hold;
        await route.fulfill({ response }); delivered();
      } else await route.continue();
    });
    const file = (name, basis) => ({ name, mimeType: 'application/octet-stream', buffer: Buffer.from(m.Request.toBinary(m.Request.create({ command: { oneofKind: 'hostImport', hostImport: m.HostImport.create({ report: { ...report, sourceBasis: basis, observedAt: 200n } }) } }))) });
    const dialog = page.getByRole('dialog', { name: 'Refresh host observations', exact: true });
    await dialog.getByLabel('Host report file').setInputFiles(file('old.omloom', 'c'.repeat(64)));
    await oldStarted;
    await dialog.getByLabel('Host report file').setInputFiles(file('current.omloom', 'a'.repeat(64)));
    const select = dialog.getByRole('combobox', { name: 'Existing source', exact: true });
    await expect(select).toHaveValue(/host-mapping\//);
    release(); await oldDelivered;
    await page.evaluate(() => new Promise(resolve => requestAnimationFrame(resolve)));
    await expect(select).toHaveValue(/host-mapping\//);
    await expect(dialog.getByRole('checkbox', { name: 'device: Late source host', exact: true })).toBeEnabled();
    await page.unrouteAll();
  });
});
