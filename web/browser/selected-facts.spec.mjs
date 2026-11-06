import { test, expect } from '@playwright/test';
import { withFieldHost } from './field-host.mjs';
import { workspaceClient } from './workspace-client.mjs';

function decode(m, route) { return m.Request.fromBinary(route.request().postDataBuffer().subarray(5)); }
async function setup(page, url, width, count = 10) {
  page.setDefaultTimeout(5000);
  const client = await workspaceClient(url); const { m, rpc } = client;
  const csv = 'external_id,name,description\n' + Array.from({ length: count }, (_, i) => `${i},Review ${String(i + 1).padStart(2, '0')},Before`).join('\n');
  const preview = (await rpc({ oneofKind: 'import', import: m.Import.create({ source: 'Selected device inspection', csv, kind: m.AssetKind.DEVICE, mappings: [{ source: 'name', field: 'name' }, { source: 'description', field: 'description' }] }) })).preview;
  const targets = (await rpc({ oneofKind: 'import', import: m.Import.create({ preview: preview.id }) })).mutation.targets;
  await page.setViewportSize({ width, height: 900 });
  await page.goto(url + '/#/?view=2');
  await expect(page.getByRole('checkbox', { name: 'Select Review 01', exact: true, includeHidden: true })).toBeAttached();
  if (width === 390) await page.getByRole('button', { name: 'Select', exact: true }).click();
  await page.getByRole('checkbox', { name: width === 390 ? 'Select phone page' : 'Select visible page', exact: true }).check();
  await page.getByRole('button', { name: 'Check selected devices', exact: true }).click();
  const dialog = page.getByRole('dialog', { name: 'Check selected devices', exact: true });
  return { ...client, targets, dialog };
}
async function question(dialog, m, field = 'drive_bays', value = '10') {
  await dialog.getByRole('combobox', { name: 'Required fact', exact: true }).selectOption(field);
  if (field === 'drive_bays') await dialog.getByRole('combobox', { name: 'Required comparison', exact: true }).selectOption(String(m.RequirementComparison.MINIMUM));
  await dialog.getByRole('textbox', { name: field === 'drive_bays' ? 'Required value (count)' : 'Required value', exact: true }).fill(value);
  await dialog.getByRole('button', { name: 'Check requirement', exact: true }).click();
  await expect(dialog.getByText('Checking selected facts…', { exact: true })).toBeHidden();
}
function row(dialog, n) { return dialog.locator('.fact-review-row').filter({ has: dialog.page().getByRole('heading', { name: `Review ${String(n).padStart(2, '0')} drive bays`, exact: true }) }); }
async function fill(editor, value) {
  await editor.getByLabel('Exact drive bays', { exact: true }).fill(value);
  await editor.getByLabel('Evidence basis for drive bays', { exact: true }).fill('Counted bays on the selected chassis');
}
for (const width of [1440, 390]) test(`selected devices retain distinct questions and advance with one fact edit at ${width}px`, async ({ page }) => {
  // The real evaluator returns two separate answers for one edit, without per-row reads or threshold input.
  await withFieldHost(async url => {
    const { m, dialog, rpc, targets, revision: readRevision } = await setup(page, url, width);
    const originalRevision = await readRevision();
    const requests = [];
    await page.route('**/omloom.workspace.v1.Workspace/Call', async route => { requests.push(decode(m, route).command); await route.continue(); });
    await question(dialog, m);
    await expect(dialog.locator('.fact-review-row')).toHaveCount(10);
    expect(requests.filter(r => r.oneofKind === 'read')).toHaveLength(0);
    const initial = requests.find(r => r.oneofKind === 'evaluateRequirements').evaluateRequirements;
    expect(initial.nodes).toHaveLength(11);
    await dialog.getByRole('button', { name: 'Add another condition' }).click();
    await question(dialog, m, 'drive_bays', '6');
    await expect(dialog.locator('.fact-review-row')).toHaveCount(10);
    await expect(row(dialog, 1).locator('.fact-conditions li')).toHaveCount(2);
    await page.screenshot({ path: test.info().outputPath(`selected-device-list-${width}.png`) });
    await row(dialog, 1).getByRole('button', { name: 'Clarify this fact', exact: true }).click();
    const editor = dialog.getByRole('region', { name: 'Selected fact editor' });
    await expect(editor.getByLabel('Exact drive bays', { exact: true })).toBeFocused();
    await expect(editor.getByLabel('Exact drive bays', { exact: true })).toHaveValue('');
    await page.keyboard.type('8'); await page.keyboard.press('Tab');
    await expect(editor.getByLabel('Evidence basis for drive bays', { exact: true })).toBeFocused();
    await page.keyboard.type('Counted bays on the selected chassis');
    await editor.getByRole('button', { name: 'Save and next', exact: true }).click();
    await expect(row(dialog, 1).locator('.fact-conditions li').nth(0)).toContainText('failed');
    await expect(row(dialog, 1).locator('.fact-conditions li').nth(1)).toContainText('satisfied');
    await expect(row(dialog, 1).locator('.fact-current')).toContainText('8');
    await expect(row(dialog, 1).locator('.fact-conditions small').first()).toContainText(`revision ${await readRevision()}`);
    await expect(row(dialog, 2).locator('.fact-conditions small').first()).toContainText(`revision ${originalRevision}`);
    await expect(editor.getByRole('region', { name: 'Review fact change' })).toContainText('Review 02');
    await expect(editor.getByLabel('Exact drive bays', { exact: true })).toBeFocused();
    const affected = requests.filter(r => r.oneofKind === 'evaluateRequirements').at(-1).evaluateRequirements;
    expect(affected.nodes).toHaveLength(3);
    expect(affected.nodes.slice(0, 2).map(n => n.expression.field.target)).toEqual([targets[0], targets[0]]);
    expect(requests.filter(r => r.oneofKind === 'write')).toHaveLength(1);
    const first = (await rpc({ oneofKind: 'read', read: m.Read.create({ target: targets[0], acceptedField: 'drive_bays' }) })).detail.item;
    expect(first.fields.find(f => f.name === 'drive_bays').value.equipment.assertion.quantity.lower).toBe(8n);
    if (width === 390) {
      const bounds = await dialog.boundingBox(); expect(bounds.width).toBe(390); expect(bounds.y).toBe(0);
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    }
    await page.screenshot({ path: test.info().outputPath(`selected-device-review-${width}.png`) });
    console.log(`Selected review ${width}px: one condition entry for ten devices, one correction, one write, automatic next-field focus.`);
  });
});

test('a conflicting row retains input while independent devices continue', async ({ page }) => {
  // A concurrent writer must not turn Save-and-next into an implicit retry or lose another row's save.
  await withFieldHost(async url => {
    const { m, rpc, targets, dialog } = await setup(page, url, 390, 3);
    await question(dialog, m);
    await row(dialog, 1).getByRole('button', { name: 'Clarify this fact', exact: true }).click();
    const editors = dialog.getByRole('region', { name: 'Selected fact editor', includeHidden: true });
    let editor = dialog.getByRole('region', { name: 'Selected fact editor' });
    await fill(editor, '8'); await editor.getByRole('button', { name: 'Save and next', exact: true }).click();
    await expect(editor.getByRole('region', { name: 'Review fact change' })).toContainText('Review 02');
    await fill(editor, '12');
    const current = (await rpc({ oneofKind: 'read', read: m.Read.create({ target: targets[1], acceptedField: 'drive_bays' }) })).detail.item;
    await rpc({ oneofKind: 'write', write: m.Write.create({ target: targets[1], kind: m.AssetKind.DEVICE, expected: current.revision, changes: [{ name: 'drive_bays', value: { oneofKind: 'integer', integer: 9n } }] }) });
    await editor.getByRole('button', { name: 'Save and next', exact: true }).click();
    await expect(editor.getByRole('button', { name: 'Read and review current fact', exact: true })).toBeVisible();
    await expect(editor.getByLabel('Exact drive bays', { exact: true })).toHaveValue('12');
    await expect(editor.getByRole('button', { name: 'Save and next', exact: true })).toBeDisabled();
    await editor.getByRole('button', { name: 'Defer this row and continue', exact: true }).click();
    editor = dialog.getByRole('region', { name: 'Selected fact editor' });
    await expect(editor.getByRole('region', { name: 'Review fact change' })).toContainText('Review 03');
    await fill(editor, '16'); await editor.getByRole('button', { name: 'Save and next', exact: true }).click();
    await expect(dialog.getByRole('region', { name: 'Selected fact editor' })).toHaveCount(0);
    await row(dialog, 2).getByRole('button', { name: 'Resume saved input', exact: true }).click();
    await expect(dialog.getByRole('region', { name: 'Selected fact editor' }).getByLabel('Exact drive bays', { exact: true })).toHaveValue('12');
    expect(await editors.count()).toBe(1);
    for (const [index, value] of [[0, 8n], [1, 9n], [2, 16n]]) {
      const item = (await rpc({ oneofKind: 'read', read: m.Read.create({ target: targets[index], acceptedField: 'drive_bays' }) })).detail.item;
      const actual = item.fields.find(f => f.name === 'drive_bays').value;
      expect(actual.oneofKind === 'integer' ? actual.integer : actual.equipment.assertion.quantity.lower).toBe(value);
    }
    const resumed = dialog.getByRole('region', { name: 'Selected fact editor' });
    await resumed.getByRole('button', { name: 'Read and review current fact', exact: true }).click();
    await expect(resumed.getByRole('region', { name: 'Review fact change' })).toContainText('Currently recorded drive bays: 9');
    await expect(resumed.getByLabel('Exact drive bays', { exact: true })).toHaveValue('12');
    await expect(resumed.getByRole('button', { name: 'Save and next', exact: true })).toBeEnabled();
    await resumed.getByRole('button', { name: 'Save and next', exact: true }).click();
    await expect(row(dialog, 2).locator('.fact-conditions')).toContainText('satisfied');
  });
});

test('a supported CSV update returns to the same selected questions', async ({ page }) => {
  // Existing personal-source mappings update the intended field, not the selected population or question.
  await withFieldHost(async url => {
    const { m, dialog, targets } = await setup(page, url, 390, 3);
    await question(dialog, m, 'description', 'Ready');
    await dialog.getByRole('button', { name: 'Import CSV and return', exact: true }).click();
    const importer = page.getByRole('dialog', { name: 'Import inventory', exact: true });
    await importer.getByLabel('Source name', { exact: true }).fill('Selected device inspection');
    await importer.getByLabel('CSV', { exact: true }).fill('external_id,description\n0,Ready\n1,Ready');
    await importer.getByLabel('CSV column', { exact: true }).fill('description');
    await importer.getByRole('combobox', { name: 'Inventory field', exact: true }).selectOption('description');
    await importer.getByRole('button', { name: 'Prepare preview', exact: true }).click();
    await expect(importer.getByRole('heading', { name: 'Review 2 rows' })).toBeVisible();
    await importer.getByRole('button', { name: 'Accept reviewed rows', exact: true }).click();
    await expect(importer).toBeHidden();
    await expect(dialog.locator('.fact-review-row')).toHaveCount(3);
    await expect(dialog.locator('.fact-conditions li').nth(0)).toContainText('satisfied');
    await expect(dialog.locator('.fact-conditions li').nth(1)).toContainText('satisfied');
    await expect(dialog.locator('.fact-conditions li').nth(2)).toContainText('failed');
    await expect(dialog.getByRole('heading', { name: '3 selected devices', exact: true })).toBeFocused();
    expect(targets).toHaveLength(3);
  });
});

for (const minimum of [6, 10]) test(`rechecking an uncertain row retains input without another write (minimum: ${minimum})`, async ({ page }) => {
  // A read cannot establish authorship of a lost write response or discard its unresolved draft.
  await withFieldHost(async url => {
    const { m, dialog } = await setup(page, url, 390, 2);
    await question(dialog, m, 'drive_bays', String(minimum));
    let writes = 0;
    await page.route('**/omloom.workspace.v1.Workspace/Call', async route => {
      const request = decode(m, route);
      if (request.command.oneofKind === 'write') { writes++; await route.fetch(); await route.abort('failed'); }
      else await route.continue();
    });
    await row(dialog, 1).getByRole('button', { name: 'Clarify this fact', exact: true }).click();
    const editor = dialog.getByRole('region', { name: 'Selected fact editor' });
    await fill(editor, '8'); await editor.getByRole('button', { name: 'Save and next', exact: true }).click();
    await expect(editor.getByText(/The write outcome is unknown/)).toBeVisible();
    await editor.getByRole('button', { name: 'Recheck without saving', exact: true }).click();
    await expect(editor.getByLabel('Exact drive bays', { exact: true })).toHaveValue('8');
    await expect(editor.getByRole('region', { name: 'Review fact change' })).toContainText('Review 01');
    expect(writes).toBe(1);
    await editor.getByRole('button', { name: 'Defer this row and continue', exact: true }).click();
    await expect(dialog.getByRole('region', { name: 'Selected fact editor' }).getByRole('region', { name: 'Review fact change' })).toContainText('Review 02');
    await dialog.getByRole('region', { name: 'Selected fact editor' }).getByRole('button', { name: 'Cancel', exact: true }).click();
    await row(dialog, 1).getByRole('button', { name: 'Resume saved input', exact: true }).click();
    await expect(dialog.getByRole('region', { name: 'Selected fact editor' }).getByLabel('Exact drive bays', { exact: true })).toHaveValue('8');
  });
});

test('ranges and zero remain distinct from unknown without repeating writes', async ({ page }) => {
  // An unchanged range stays partial knowledge; zero is a real fact rather than an empty editor.
  await withFieldHost(async url => {
    const { m, dialog, rpc, targets } = await setup(page, url, 390, 3);
    const range = { name: 'drive_bays', value: { oneofKind: 'equipment', equipment: { basis: 'Inspection establishes only eight to twelve bays', assertion: { oneofKind: 'quantity', quantity: { lower: 8n, upper: 12n, unit: 'count', scale: 0 } } } } };
    for (const [i, field] of [[0, range], [1, { name: 'drive_bays', value: { oneofKind: 'integer', integer: 0n } }]]) {
      const current = (await rpc({ oneofKind: 'read', read: m.Read.create({ target: targets[i], acceptedField: 'drive_bays' }) })).detail.item;
      await rpc({ oneofKind: 'write', write: m.Write.create({ target: targets[i], expected: current.revision, kind: m.AssetKind.DEVICE, changes: [field] }) });
    }
    let writes = 0;
    await page.route('**/omloom.workspace.v1.Workspace/Call', async route => { if (decode(m, route).command.oneofKind === 'write') writes++; await route.continue(); });
    await question(dialog, m);
    await expect(row(dialog, 1).locator('.fact-current')).toContainText('8–12 bays');
    await expect(row(dialog, 1).locator('.fact-conditions')).toContainText('unknown');
    await expect(row(dialog, 2).locator('.fact-current')).toContainText('0');
    await expect(row(dialog, 2).locator('.fact-conditions')).toContainText('failed');
    await expect(row(dialog, 3).locator('.fact-current')).toContainText('Unknown');
    await row(dialog, 1).getByRole('button', { name: 'Clarify this fact', exact: true }).click();
    const editor = dialog.getByRole('region', { name: 'Selected fact editor' });
    await editor.getByRole('button', { name: 'Save and next', exact: true }).click();
    await expect(editor.getByRole('region', { name: 'Review fact change' })).toContainText('Review 02');
    await expect(editor.getByLabel('Known drive bays', { exact: true })).toHaveValue('0');
    const actual = (await rpc({ oneofKind: 'read', read: m.Read.create({ target: targets[0], acceptedField: 'drive_bays' }) })).detail.item.fields.find(f => f.name === 'drive_bays');
    expect(actual).toEqual(range);
    expect(writes).toBe(0);
  });
});

test('an existing finding opens the selected review without another question entry', async ({ page }) => {
  // Findings are passed through the same typed field-condition contract, not a new finding registry.
  await withFieldHost(async url => {
    const { m, rpc } = await workspaceClient(url);
    const target = (await rpc({ oneofKind: 'write', write: m.Write.create({ kind: m.AssetKind.DEVICE, changes: [{ name: 'name', value: { oneofKind: 'text', text: 'Finding device' } }] }) })).mutation.targets[0];
    await page.goto(`${url}/#/?asset=${target.assetId}&incarnation=${target.incarnation}`);
    const panel = page.locator('.requirements-panel');
    await panel.getByText('Check recorded requirements', { exact: true }).click();
    await question(panel, m);
    await panel.getByRole('button', { name: 'Review these findings', exact: true }).click();
    const dialog = page.getByRole('dialog', { name: 'Check selected devices', exact: true });
    await expect(dialog.locator('.fact-review-row')).toHaveCount(1);
    await expect(dialog.locator('.fact-conditions')).toContainText('10');
    await expect(dialog.getByRole('combobox', { name: 'Required fact', exact: true })).toHaveCount(0);
  });
});

test('list changes and newly discovered devices cannot expand the reviewed selection', async ({ page }) => {
  // The list may refresh underneath a review, but its conditions and exact targets remain fixed.
  await withFieldHost(async url => {
    const { m, dialog, rpc, targets } = await setup(page, url, 390, 3);
    await question(dialog, m);
    await rpc({ oneofKind: 'write', write: m.Write.create({ kind: m.AssetKind.DEVICE, changes: [{ name: 'name', value: { oneofKind: 'text', text: 'New discovery' } }] }) });
    await page.evaluate(() => { location.hash = '/?view=2&search=Review%2003'; });
    const queries = [];
    await page.route('**/omloom.workspace.v1.Workspace/Call', async route => { const request = decode(m, route); if (request.command.oneofKind === 'evaluateRequirements') queries.push(request.command.evaluateRequirements); await route.continue(); });
    await dialog.getByRole('button', { name: 'Refresh checks', exact: true }).click();
    await expect(dialog.getByText('Checking selected facts…', { exact: true })).toBeHidden();
    await expect(dialog.locator('.fact-review-row')).toHaveCount(3);
    expect(queries).toHaveLength(1);
    expect(queries[0].nodes.slice(0, 3).map(node => node.expression.field.target)).toEqual(targets);
    await expect(dialog).not.toContainText('New discovery');
    await expect(dialog.locator('.fact-conditions li').first()).toContainText('10');
  });
});

test('selected review measures question entry and navigation against individual cards', async ({ page }) => {
  // Compare the same ten-device read-only question on the actual individual and selected journeys.
  await withFieldHost(async url => {
    const { m, dialog, targets } = await setup(page, url, 1440);
    await dialog.getByRole('button', { name: 'Close dialog', exact: true }).click();
    const evaluated = [];
    await page.route('**/omloom.workspace.v1.Workspace/Call', async route => {
      const command = decode(m, route).command;
      if (command.oneofKind === 'evaluateRequirements') evaluated.push(command.evaluateRequirements);
      await route.continue();
    });
    const individualStart = Date.now();
    for (const target of targets) {
      await page.goto(`${url}/#/?view=2&asset=${target.assetId}&incarnation=${target.incarnation}`);
      const panel = page.locator('.requirements-panel');
      await panel.getByText('Check recorded requirements', { exact: true }).click();
      await question(panel, m);
      await expect(panel.getByRole('heading', { name: 'Requirement not established', exact: true })).toBeVisible();
    }
    const individualMs = Date.now() - individualStart;
    expect(evaluated).toHaveLength(10);
    expect(evaluated.every(query => query.nodes.length === 1)).toBe(true);
    const selectedStart = Date.now();
    await page.goto(url + '/#/?view=2');
    await page.getByRole('checkbox', { name: 'Select visible page', exact: true }).check();
    await page.getByRole('button', { name: 'Check selected devices', exact: true }).click();
    const review = page.getByRole('dialog', { name: 'Check selected devices', exact: true });
    await question(review, m);
    await expect(review.locator('.fact-conditions strong')).toHaveText(Array(10).fill('unknown'));
    expect(evaluated).toHaveLength(11);
    expect(evaluated[10].nodes).toHaveLength(11);
    console.log(JSON.stringify({ individual: { devices: 10, cardVisits: 10, questionEntries: 10, elapsedMs: individualMs }, selected: { devices: 10, cardVisits: 0, questionEntries: 1, elapsedMs: Date.now() - selectedStart } }));
  });
});
