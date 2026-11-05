import { test, expect } from '@playwright/test';
import { withFieldHost } from './field-host.mjs';
import { workspaceClient } from './workspace-client.mjs';

for (const width of [1440, 390]) test(`anonymous host comparison and explicit association at ${width}px`, async ({ page }) => {
  // A real same-model fleet must remain ambiguous. Only an explicit comparison
  // decision associates the report, and the next anonymous file starts unresolved.
  await withFieldHost(async url => {
    const { m, rpc } = await workspaceClient(url);
    const text = (name, value) => ({ name, value: { oneofKind: 'text', text: value } });
    const hosts = new Map();
    for (let index = 0; index < 27; index++) {
      const name = `Fleet ${index}`;
      const value = await rpc({ oneofKind: 'write', write: m.Write.create({ kind: m.AssetKind.DEVICE, changes: [text('name', name), text('model', 'Same chassis'), text('serial', `serial-${index}`)] }) });
      hosts.set(name, value.mutation.targets[0]);
    }
    const report = m.HostReport.create({ version: 1, platform: 'linux', bootBasis: 'b'.repeat(64), observedAt: 100n, findings: [{ key: 'hardware', kind: m.AssetKind.DEVICE, fields: [text('name', 'Anonymous execution host'), text('model', 'Same chassis'), text('serial', 'new serial')] }] });
    const file = () => ({ name: 'same-name.omloom', mimeType: 'application/octet-stream', buffer: Buffer.from(m.Request.toBinary(m.Request.create({ command: { oneofKind: 'hostImport', hostImport: m.HostImport.create({ report }) } }))) });
    await page.setViewportSize({ width, height: 900 });
    await page.goto(url);
    if (width === 390) await page.getByRole('button', { name: 'More', exact: true }).click();
    await page.getByRole('button', { name: 'Import host report', exact: true }).click();
    const dialog = page.getByRole('dialog', { name: 'Import host observations', exact: true });
    await dialog.getByLabel('Host report file').setInputFiles(file());
    await dialog.getByLabel('Source name', { exact: true }).fill('Manual collector');
    await dialog.getByRole('checkbox', { name: 'device: Anonymous execution host', exact: true }).check();
    await expect(dialog.getByRole('button', { name: 'Prepare host preview', exact: true })).toBeDisabled();
    await dialog.getByRole('button', { name: 'Compare possible matches', exact: true }).click();
    await expect(dialog.locator('.identity-candidate')).toHaveCount(25);
    await expect(dialog.getByText('Difference: serial: different declared value; replacement, upgrade or reinstall may explain it', { exact: true })).toHaveCount(25);
    await dialog.locator('.identity-candidate').nth(0).getByRole('button', { name: 'Compare candidate', exact: true }).click();
    await dialog.locator('.identity-candidate').nth(1).getByRole('button', { name: 'Compare candidate', exact: true }).click();
    const comparison = dialog.getByRole('region', { name: 'Compare observation candidates' });
    await expect(comparison.locator('article')).toHaveCount(2);
    const name = (await comparison.locator('article h5').first().innerText()).replace(/^device: /, '');
    const target = hosts.get(name);
    expect(target).toBeDefined();
    await comparison.getByRole('button', { name: 'Confirm this existing resource', exact: true }).first().click();
    await rpc({ oneofKind: 'write', write: m.Write.create({ kind: m.AssetKind.DEVICE, changes: [text('name', 'Concurrent unrelated device')] }) });
    await dialog.getByRole('button', { name: 'Prepare host preview', exact: true }).click();
    await expect(dialog.getByRole('alert')).toContainText('Review the package and selected matches again');
    await expect(dialog.getByRole('checkbox', { name: 'device: Anonymous execution host', exact: true })).toBeChecked();
    await dialog.getByRole('button', { name: 'Compare possible matches', exact: true }).click();
    await dialog.locator('.identity-candidate').first().getByRole('button', { name: 'Compare candidate', exact: true }).click();
    await comparison.getByRole('button', { name: 'Confirm this existing resource', exact: true }).click();
    await dialog.getByRole('button', { name: 'Prepare host preview', exact: true }).click();
    await expect(dialog.getByRole('heading', { name: 'Review 1 selected resources', exact: true })).toBeVisible();
    await dialog.getByRole('button', { name: 'Back', exact: true }).click();
    await expect(dialog.getByText(`Existing match: ${name}`, { exact: true })).toBeVisible();
    await dialog.getByRole('button', { name: 'Prepare host preview', exact: true }).click();
    await dialog.getByRole('button', { name: 'Accept selected observations', exact: true }).click();
    await expect(dialog).toBeHidden();
    const detail = (await rpc({ oneofKind: 'read', read: m.Read.create({ target }) })).detail;
    expect(detail.item.fields.find(v => v.name === 'name').value.text).toBe(name);
    expect(detail.observed[0].evidence.fields.find(v => v.name === 'name').value.text).toBe('Anonymous execution host');
    report.observedAt = 200n;
    if (width === 390) await page.getByRole('button', { name: 'More', exact: true }).click();
    await page.getByRole('button', { name: 'Import host report', exact: true }).click();
    await dialog.getByLabel('Host report file').setInputFiles(file());
    await expect(dialog.getByText(/^Existing match:/)).toHaveCount(0);
    await dialog.getByRole('button', { name: 'Leave unresolved', exact: true }).click();
    await expect(dialog.getByRole('checkbox', { name: 'device: Anonymous execution host', exact: true })).not.toBeChecked();
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    await dialog.getByRole('button', { name: 'Cancel', exact: true }).click();
  });
});

for (const width of [1440, 390]) test(`unqualified host package enters manual matching at ${width}px`, async ({ page }) => {
  // Missing continuity is not permission to adopt another chassis. The user
  // explicitly confirms the already opened host through the same importer.
  await withFieldHost(async url => {
    const { m, rpc } = await workspaceClient(url);
    const field = value => ({ name: 'name', value: { oneofKind: 'text', text: value } });
    const target = (await rpc({ oneofKind: 'write', write: m.Write.create({ kind: m.AssetKind.DEVICE, changes: [field('Known host')] }) })).mutation.targets[0];
    const report = m.HostReport.create({ version: 1, platform: 'linux', bootBasis: 'b'.repeat(64), observedAt: 100n, findings: [{ key: 'hardware', kind: m.AssetKind.DEVICE, fields: [field('Anonymous observation')] }] });
    await page.setViewportSize({ width, height: 900 });
    await page.goto(`${url}/#/?asset=${target.assetId}&incarnation=${target.incarnation}`);
    await page.getByRole('button', { name: 'Load observation package', exact: true }).click();
    await page.getByLabel('Host report file').setInputFiles({ name: 'anonymous.omloom', mimeType: 'application/octet-stream', buffer: Buffer.from(m.Request.toBinary(m.Request.create({ command: { oneofKind: 'hostImport', hostImport: m.HostImport.create({ report }) } }))) });
    await page.getByRole('button', { name: 'Match this package manually', exact: true }).click();
    const dialog = page.getByRole('dialog', { name: 'Import host observations', exact: true });
    await dialog.getByLabel('Source name', { exact: true }).fill('Manual observation');
    await dialog.getByRole('button', { name: 'Use current device or VM scope', exact: true }).click();
    await expect(dialog.getByText('Existing match: Known host', { exact: true })).toBeVisible();
    await dialog.getByRole('checkbox', { name: 'device: Anonymous observation', exact: true }).check();
    await dialog.getByRole('button', { name: 'Prepare host preview', exact: true }).click();
    await expect(dialog.getByRole('heading', { name: 'Review 1 selected resources', exact: true })).toBeVisible();
    await dialog.getByRole('button', { name: 'Accept selected observations', exact: true }).click();
    await expect(dialog).toBeHidden();
    const detail = (await rpc({ oneofKind: 'read', read: m.Read.create({ target }) })).detail;
    expect(detail.item.fields).toEqual([field('Known host')]);
    expect(detail.observed[0].evidence.fields).toEqual([field('Anonymous observation')]);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  });
});
