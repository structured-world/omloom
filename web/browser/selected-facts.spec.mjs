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
    const deferred = row(dialog, 2).getByText('Deferred, input retained.', { exact: true });
    await expect(deferred).toBeVisible();
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
    await resumed.getByRole('button', { name: 'Recheck without saving', exact: true }).click();
    await expect(resumed.getByLabel('Exact drive bays', { exact: true })).toHaveValue('12');
    await expect(resumed.getByRole('region', { name: 'Review fact change' })).toContainText('Review 02');
    await resumed.getByRole('button', { name: 'Save and next', exact: true }).click();
    await expect(row(dialog, 2).locator('.fact-conditions')).toContainText('satisfied');
    await expect(deferred).toHaveCount(0);
  });
});

test('canceling a resumed draft makes its row eligible for save and next', async ({ page }) => {
  // Only a retained inactive draft can defer a row from automatic advancement.
  await withFieldHost(async url => {
    const { m, dialog } = await setup(page, url, 1440, 3);
    await question(dialog, m);
    const second = row(dialog, 2);
    const editor = dialog.getByRole('region', { name: 'Selected fact editor' });
    const deferred = second.getByText('Deferred, input retained.', { exact: true });
    await second.getByRole('button', { name: 'Clarify this fact', exact: true }).click();
    await fill(editor, '13');
    await editor.getByRole('button', { name: 'Defer this row and continue', exact: true }).click();
    await expect(editor.getByRole('region', { name: 'Review fact change' })).toContainText('Review 03');
    await editor.getByRole('button', { name: 'Cancel', exact: true }).click();
    await expect(deferred).toBeVisible();
    await second.getByRole('button', { name: 'Resume saved input', exact: true }).click();
    await expect(editor.getByLabel('Exact drive bays', { exact: true })).toHaveValue('13');
    await expect(deferred).toHaveCount(0);
    await editor.getByRole('button', { name: 'Cancel', exact: true }).click();
    await expect(deferred).toHaveCount(0);
    await row(dialog, 1).getByRole('button', { name: 'Clarify this fact', exact: true }).click();
    await fill(editor, '12');
    await editor.getByRole('button', { name: 'Save and next', exact: true }).click();
    await expect(editor.getByRole('region', { name: 'Review fact change' })).toContainText('Review 02');
    await expect(editor.getByLabel('Exact drive bays', { exact: true })).toHaveValue('');
  });
});

test('an explicit empty description differs from clear and another value', async ({ page }) => {
  // Empty is a real text condition; missing/cleared evidence cannot satisfy it.
  await withFieldHost(async url => {
    const { m, rpc, targets, dialog } = await setup(page, url, 390, 3);
    for (const [i, value] of [[0, { oneofKind: 'text', text: '' }], [1, { oneofKind: 'clear', clear: m.NullValue.NULL_VALUE }]]) {
      const item = (await rpc({ oneofKind: 'read', read: m.Read.create({ target: targets[i], acceptedField: 'description' }) })).detail.item;
      await rpc({ oneofKind: 'write', write: m.Write.create({ target: targets[i], kind: m.AssetKind.DEVICE, expected: item.revision, changes: [{ name: 'description', value }] }) });
    }
    await question(dialog, m, 'description', '');
    const rows = dialog.locator('.fact-review-row');
    await expect(rows.nth(0).locator('.fact-conditions span')).toHaveText('description equals (empty) (accepted configuration)');
    await expect(rows.nth(0).locator('.fact-current')).toContainText('(empty)');
    await expect(rows.nth(0).locator('.fact-conditions')).toContainText('satisfied');
    await expect(rows.nth(1).locator('.fact-conditions')).toContainText('unknown');
    await expect(rows.nth(2).locator('.fact-conditions')).toContainText('failed');
  });
});

for (const width of [1440, 390]) test(`review one shared fact and apply only the included devices at ${width}px`, async ({ page }) => {
  // The shared editor uses one value, one bounded preview and one atomic bulk command.
  await withFieldHost(async url => {
    const { m, rpc, targets, dialog } = await setup(page, url, width, 3);
    await question(dialog, m);
    const entry = dialog.getByRole('button', { name: 'Apply shared drive bays', exact: true });
    await entry.focus(); await page.keyboard.press('Enter');
    const shared = dialog.getByRole('region', { name: 'Shared fact' });
    await expect(shared.getByRole('heading', { name: 'Apply one value for drive bays' })).toBeFocused();
    await shared.getByRole('checkbox', { name: /Review 02/ }).uncheck();
    await fill(shared, '0');
    const evaluations = []; const writes = [];
    await page.route('**/omloom.workspace.v1.Workspace/Call', route => {
      const command = decode(m, route).command;
      if (command.oneofKind === 'evaluateRequirements') evaluations.push(command.evaluateRequirements.nodes.length - 1);
      if (command.oneofKind === 'bulk') writes.push(command.bulk);
      return route.continue();
    });
    await shared.getByRole('button', { name: 'Review shared change' }).click();
    await expect(shared.getByRole('heading', { name: 'Review 2 included devices' })).toBeFocused();
    await expect(shared.getByText('Before: Unknown', { exact: true })).toHaveCount(2);
    await expect(shared.getByText(/After:.*0 bays.*Counted bays/)).toHaveCount(2);
    const apply = shared.getByRole('button', { name: 'Apply to reviewed devices' });
    await apply.focus(); await page.keyboard.press('Enter');
    await expect(dialog.getByRole('heading', { name: '3 selected devices', exact: true })).toBeFocused();
    expect(writes).toHaveLength(1);
    expect(writes[0].sharedFact).toBe(true);
    expect(writes[0].targets).toEqual([targets[0], targets[2]]);
    expect(evaluations).toEqual([2, 2]);
    for (const index of [0, 1, 2]) {
      const current = (await rpc({ oneofKind: 'read', read: m.Read.create({ target: targets[index], acceptedField: 'drive_bays' }) })).detail.item;
      const field = current.fields.find(field => field.name === 'drive_bays');
      if (index === 1) expect(field).toBeUndefined();
      else expect(field.value.equipment.assertion.quantity.lower).toBe(0n);
    }
  });
});

test('a stale shared review keeps input and requires an explicit narrowed review', async ({ page }) => {
  // An intervening individual save changes the reviewed revision and must survive rejection.
  await withFieldHost(async url => {
    const { m, rpc, targets, dialog } = await setup(page, url, 390, 3);
    await question(dialog, m);
    await dialog.getByRole('button', { name: 'Apply shared drive bays', exact: true }).click();
    const shared = dialog.getByRole('region', { name: 'Shared fact' });
    await fill(shared, '14');
    await shared.getByRole('button', { name: 'Review shared change' }).click();
    const apply = shared.getByRole('button', { name: 'Apply to reviewed devices' });
    await expect(apply).toBeEnabled();
    const read = async index => (await rpc({ oneofKind: 'read', read: m.Read.create({ target: targets[index], acceptedField: 'drive_bays' }) })).detail.item;
    await rpc({ oneofKind: 'write', write: m.Write.create({ target: targets[1], kind: m.AssetKind.DEVICE,
      expected: (await read(1)).revision, changes: [{ name: 'drive_bays', value: { oneofKind: 'integer', integer: 7n } }],
    }) });
    await apply.click();
    await expect(shared.getByRole('alert')).toBeVisible();
    await expect(shared.getByLabel('Exact drive bays')).toHaveValue('14');
    expect((await read(0)).fields.find(field => field.name === 'drive_bays')).toBeUndefined();
    await shared.getByRole('checkbox', { name: /Review 02/ }).uncheck();
    await expect(apply).toHaveCount(0);
    await shared.getByRole('button', { name: 'Review shared change' }).click();
    await apply.click();
    await expect(shared).toHaveCount(0);
    expect((await read(1)).fields.find(field => field.name === 'drive_bays').value.integer).toBe(7n);
    expect((await read(0)).fields.find(field => field.name === 'drive_bays').value.equipment.assertion.quantity.lower).toBe(14n);
  });
});

test('a newly restricted member remains selected until explicitly excluded', async ({ page }) => {
  // Use the restricted leaf shape returned by the store when field access is lost.
  await withFieldHost(async url => {
    const { m, targets, dialog } = await setup(page, url, 390, 3);
    await question(dialog, m);
    await dialog.getByRole('button', { name: 'Apply shared drive bays', exact: true }).click();
    const shared = dialog.getByRole('region', { name: 'Shared fact' });
    await fill(shared, '11');
    const writes = [];
    await page.route('**/omloom.workspace.v1.Workspace/Call', async route => {
      const command = decode(m, route).command;
      if (command.oneofKind === 'bulk') writes.push(command.bulk.targets);
      if (command.oneofKind !== 'evaluateRequirements') return route.continue();
      const index = command.evaluateRequirements.nodes.findIndex(node => node.expression.oneofKind === 'field'
        && node.expression.field.target.assetId === targets[1].assetId);
      if (index < 0) return route.continue();
      const response = await route.fetch();
      const body = await response.body(); const length = body.readUInt32BE(1);
      const reply = m.Reply.fromBinary(body.subarray(5, 5 + length));
      const denied = reply.result.requirementEvaluation.results.find(result => result.node === index);
      denied.reason = m.RequirementReason.RESTRICTED;
      denied.target = undefined; denied.field = undefined; denied.fieldWritable = false;
      denied.evidence = []; denied.resolutions = [];
      const output = m.Reply.toBinary(reply); const frame = Buffer.alloc(output.length + 5);
      frame.writeUInt32BE(output.length, 1); frame.set(output, 5);
      await route.fulfill({ response, body: Buffer.concat([frame, body.subarray(5 + length)]) });
    });
    await shared.getByRole('button', { name: 'Review shared change' }).click();
    const apply = shared.getByRole('button', { name: 'Apply to reviewed devices' });
    const member = shared.getByRole('checkbox', { name: 'Device 2 (unavailable)' });
    await expect(member).toBeChecked();
    await expect(apply).toBeDisabled();
    await expect(shared).not.toContainText('Review 02');
    await member.uncheck();
    await expect(apply).toHaveCount(0);
    await shared.getByRole('button', { name: 'Review shared change' }).click();
    await apply.click();
    await expect(dialog.getByRole('heading', { name: 'Unavailable device fact', exact: true })).toHaveCount(1);
    expect(writes).toEqual([[targets[0], targets[2]]]);
  });
});

test('an unconfirmed shared save can read back facts without another bulk command', async ({ page }) => {
  // Drop the reply only after the real disk-backed mutation completed.
  await withFieldHost(async url => {
    const { m, dialog } = await setup(page, url, 390, 2);
    await question(dialog, m);
    await dialog.getByRole('button', { name: 'Apply shared drive bays', exact: true }).click();
    const shared = dialog.getByRole('region', { name: 'Shared fact' });
    await fill(shared, '14');
    await shared.getByRole('button', { name: 'Review shared change' }).click();
    let writes = 0;
    await page.route('**/omloom.workspace.v1.Workspace/Call', async route => {
      if (decode(m, route).command.oneofKind !== 'bulk') return route.continue();
      writes++;
      const response = await route.fetch();
      const bytes = await response.body();
      expect(m.Reply.fromBinary(bytes.subarray(5, 5 + bytes.readUInt32BE(1))).result.oneofKind).toBe('mutation');
      await route.fulfill({ status: 503, body: 'Response lost' });
    });
    await shared.getByRole('button', { name: 'Apply to reviewed devices' }).click();
    await expect(shared.getByRole('alert')).toContainText('Save not confirmed');
    await expect(shared.getByLabel('Exact drive bays')).toHaveValue('14');
    await shared.getByRole('button', { name: 'Review current values' }).click();
    await expect(shared.getByText(/Before:.*14 bays/)).toHaveCount(2);
    await expect(shared.getByRole('status')).toContainText('cannot confirm who made');
    await shared.getByRole('button', { name: 'Back to checks' }).click();
    await expect(row(dialog, 1).locator('.fact-conditions')).toContainText('satisfied');
    expect(writes).toBe(1);
  });
});

test('a shared empty description is an accepted value distinct from explicit clear', async ({ page }) => {
  // The shared form never turns blank input into an implicit clear.
  await withFieldHost(async url => {
    const { m, rpc, targets, dialog } = await setup(page, url, 1440, 2);
    await question(dialog, m, 'description', '');
    const shared = dialog.getByRole('region', { name: 'Shared fact' });
    for (const [mode, label, kind] of [['set', '(empty)', 'text'], ['clear', '(cleared)', 'clear']]) {
      await dialog.getByRole('button', { name: 'Apply shared description', exact: true }).click();
      const review = shared.getByRole('button', { name: 'Review shared change' });
      await expect(review).toBeDisabled();
      await shared.getByRole('combobox', { name: 'Change' }).selectOption(mode);
      await review.click();
      await expect(shared.getByText(`After: ${label}`, { exact: true })).toHaveCount(2);
      await shared.getByRole('button', { name: 'Apply to reviewed devices' }).click();
      await expect(dialog.getByRole('heading', { name: '2 selected devices', exact: true })).toBeFocused();
      for (const target of targets) {
        const item = (await rpc({ oneofKind: 'read', read: m.Read.create({ target, acceptedField: 'description' }) })).detail.item;
        const value = item.fields.find(field => field.name === 'description').value;
        expect(value.oneofKind).toBe(kind);
        if (kind === 'text') expect(value.text).toBe('');
      }
    }
  });
});

test('shared assignment enters one value and uses one write for three devices', async ({ page }) => {
  // Compare the same confirmed result through individual and shared editing.
  const measurements = {};
  for (const shared of [false, true]) await withFieldHost(async url => {
    const { m, dialog } = await setup(page, url, 1440, 3);
    await question(dialog, m);
    const calls = [];
    const endpoint = '**/omloom.workspace.v1.Workspace/Call';
    const count = route => { calls.push(decode(m, route).command.oneofKind); return route.continue(); };
    await page.route(endpoint, count);
    let entries = 0;
    const start = performance.now();
    if (shared) {
      await dialog.getByRole('button', { name: 'Apply shared drive bays', exact: true }).click();
      const editor = dialog.getByRole('region', { name: 'Shared fact' });
      await fill(editor, '12'); entries++;
      await editor.getByRole('button', { name: 'Review shared change' }).click();
      await editor.getByRole('button', { name: 'Apply to reviewed devices' }).click();
    } else {
      await row(dialog, 1).getByRole('button', { name: 'Clarify this fact', exact: true }).click();
      const editor = dialog.getByRole('region', { name: 'Selected fact editor' });
      for (const index of [1, 2, 3]) {
        await expect(editor.getByRole('region', { name: 'Review fact change' })).toContainText(`Review 0${index}`);
        await fill(editor, '12'); entries++;
        await editor.getByRole('button', { name: 'Save and next', exact: true }).click();
      }
    }
    for (const index of [1, 2, 3]) await expect(row(dialog, index).locator('.fact-conditions')).toContainText('satisfied');
    measurements[shared ? 'shared' : 'individual'] = { entries, writes: calls.filter(call => call === 'write' || call === 'bulk').length,
      acceptedReads: calls.filter(call => call === 'read').length,
      evaluations: calls.filter(call => call === 'evaluateRequirements').length, elapsedMs: Math.round(performance.now() - start) };
    await page.unroute(endpoint, count);
  });
  expect(measurements.individual).toMatchObject({ entries: 3, writes: 3, acceptedReads: 3, evaluations: 3 });
  expect(measurements.shared).toMatchObject({ entries: 1, writes: 1, acceptedReads: 0, evaluations: 2 });
  console.log(JSON.stringify(measurements));
});

for (const failure of ['scope', 'evidence-change', 'unavailable', 'partial-scope', 'partial-recovered', 'partial-unavailable', 'anchor']) test(`refresh handles ${failure} without confusing revocation and retry`, async ({ page }) => {
  // Actual scope/continuation failures and a transport outage have different draft consequences.
  await withFieldHost(async url => {
    const { m, rpc, targets, dialog } = await setup(page, url, 390, 3);
    const other = (await rpc({ oneofKind: 'write', write: m.Write.create({ kind: m.AssetKind.DEVICE, changes: [{ name: 'name', value: { oneofKind: 'text', text: 'Outside context' } }] }) })).mutation.targets[0];
    await question(dialog, m);
    await row(dialog, 1).getByRole('button', { name: 'Clarify this fact', exact: true }).click();
    await fill(dialog.getByRole('region', { name: 'Selected fact editor' }), '16');
    await dialog.getByRole('button', { name: 'Save and next', exact: true }).click();
    await expect(dialog.getByText('Saved drive bays for Review 01. Checking the original conditions.', { exact: true })).toBeVisible();
    await fill(dialog.getByRole('region', { name: 'Selected fact editor' }), '8');
    await dialog.getByRole('button', { name: 'Defer this row and continue', exact: true }).click();
    await fill(dialog.getByRole('region', { name: 'Selected fact editor' }), '12');
    const partial = failure.startsWith('partial-');
    const checks = [];
    await page.route('**/omloom.workspace.v1.Workspace/Call', async route => {
      const request = decode(m, route);
      if (request.command.oneofKind !== 'evaluateRequirements') return route.continue();
      const query = request.command.evaluateRequirements;
      checks.push(query.nodes.length - 1);
      if (failure === 'unavailable') return route.abort('connectionreset');
      if (failure === 'partial-unavailable' && checks.length === 2) return route.abort('connectionreset');
      if (failure === 'scope' || failure === 'partial-scope' || (partial && checks.length === 1)) {
        request.working = m.WorkingContext.create({ roots: [other], focus: other });
      } else if (failure === 'anchor') {
        if (query.anchor.assetId === targets[0].assetId) query.anchor.assetId = other.incarnation;
        query.nodes[0].expression.field.target.assetId = other.incarnation;
      } else if (failure === 'partial-recovered') {
        query.nodes[2].expression.field.target.incarnation = other.incarnation;
      } else if (failure === 'evidence-change') {
        query.cursor = '0'.repeat(64) + ':1';
      }
      const bytes = m.Request.toBinary(request); const frame = Buffer.alloc(bytes.length + 5);
      frame.writeUInt32BE(bytes.length, 1); frame.set(bytes, 5);
      await route.continue({ postData: frame });
    });
    if (partial) await dialog.getByRole('button', { name: 'Save and next', exact: true }).click();
    else await dialog.getByRole('button', { name: 'Refresh checks', exact: true }).click();
    if (partial) {
      await expect.poll(() => checks).toEqual(failure === 'partial-scope' ? [1, 3, 3, 3] : [1, 3]);
      await expect(dialog.getByText('Checking selected facts…', { exact: true })).toBeHidden();
    }
    if (failure === 'anchor') {
      await expect(dialog.getByRole('heading', { name: 'Unavailable device fact', exact: true })).toHaveCount(1);
      await expect(dialog.getByRole('region', { name: 'Selected fact editor' }).getByLabel('Exact drive bays', { exact: true })).toHaveValue('12');
      expect(checks).toEqual([3, 3]);
      await dialog.getByRole('button', { name: 'Refresh checks', exact: true }).click();
      await expect(dialog.getByText('Checking selected facts…', { exact: true })).toBeHidden();
      expect(checks).toEqual([3, 3, 3]);
      await dialog.getByRole('region', { name: 'Selected fact editor' }).getByRole('button', { name: 'Cancel', exact: true }).click();
      await row(dialog, 2).getByRole('button', { name: 'Resume saved input', exact: true }).click();
      await expect(dialog.getByRole('region', { name: 'Selected fact editor' }).getByLabel('Exact drive bays', { exact: true })).toHaveValue('8');
      return;
    }
    if (failure === 'partial-recovered' || failure === 'partial-unavailable') {
      if (failure === 'partial-unavailable') {
        await expect(dialog.getByRole('alert')).toBeVisible();
        await expect(dialog.locator('.fact-review-row')).toHaveCount(0);
        await expect(dialog.getByText('Access to this review could not be verified. Refresh checks to continue.', { exact: true })).toBeVisible();
        await expect(dialog.getByRole('region', { name: 'Selected fact editor' })).toHaveCount(0);
        await expect(dialog.getByRole('button', { name: 'Resume saved input', exact: true })).toHaveCount(0);
        await dialog.getByRole('button', { name: 'Refresh checks', exact: true }).click();
      } else {
        await expect(dialog.getByRole('heading', { name: 'Unavailable device fact', exact: true })).toHaveCount(1);
      }
      await expect(row(dialog, 1).locator('.fact-current')).toContainText('16');
      await row(dialog, 2).getByRole('button', { name: 'Resume saved input', exact: true }).click();
      await expect(dialog.getByRole('region', { name: 'Selected fact editor' }).getByLabel('Exact drive bays', { exact: true })).toHaveValue('8');
      return;
    }
    await expect(dialog.getByRole('alert')).toBeVisible();
    if (failure === 'scope' || failure === 'partial-scope') {
      await expect(dialog.getByRole('region', { name: 'Selected fact editor', includeHidden: true })).toHaveCount(0);
      await expect(dialog.getByRole('heading', { name: 'Unavailable device fact', exact: true })).toHaveCount(3);
      await expect(dialog.getByRole('button', { name: 'Resume saved input', exact: true })).toHaveCount(0);
      await expect(dialog).not.toContainText('Review 01');
    } else {
      await expect(dialog.getByRole('region', { name: 'Selected fact editor' }).getByLabel('Exact drive bays', { exact: true })).toHaveValue('12');
      await dialog.getByRole('region', { name: 'Selected fact editor' }).getByRole('button', { name: 'Cancel', exact: true }).click();
      await row(dialog, 2).getByRole('button', { name: 'Resume saved input', exact: true }).click();
      await expect(dialog.getByRole('region', { name: 'Selected fact editor' }).getByLabel('Exact drive bays', { exact: true })).toHaveValue('8');
    }
  });
});

test('a restricted observed fact hides accepted evidence and drops its draft', async ({ page }) => {
  // Both evidence layers share one field-read permission.
  await withFieldHost(async url => {
    const { m, rpc, targets, dialog } = await setup(page, url, 1440, 2);
    const target = targets[1];
    const item = (await rpc({ oneofKind: 'read', read: m.Read.create({ target, acceptedField: 'drive_bays' }) })).detail.item;
    await rpc({ oneofKind: 'write', write: m.Write.create({ target, kind: m.AssetKind.DEVICE, expected: item.revision, changes: [{ name: 'drive_bays', value: { oneofKind: 'integer', integer: 7n } }] }) });
    await question(dialog, m);
    await expect(row(dialog, 2).locator('.fact-current')).toContainText('7');
    await row(dialog, 2).getByRole('button', { name: 'Correct recorded fact', exact: true }).click();
    await dialog.getByRole('region', { name: 'Selected fact editor' }).getByLabel('Known drive bays', { exact: true }).fill('13');
    await dialog.getByRole('button', { name: 'Defer this row and continue', exact: true }).click();
    await page.route('**/omloom.workspace.v1.Workspace/Call', async route => {
      const request = decode(m, route);
      if (request.command.oneofKind !== 'evaluateRequirements') return route.continue();
      const response = await route.fetch();
      const body = await response.body(); const length = body.readUInt32BE(1);
      const reply = m.Reply.fromBinary(body.subarray(5, 5 + length));
      const evaluation = reply.result.requirementEvaluation;
      const index = request.command.evaluateRequirements.nodes.findIndex(node =>
        node.expression.oneofKind === 'field' && node.expression.field.target.assetId === target.assetId);
      const denied = evaluation.results.find(entry => entry.node === index);
      denied.reason = m.RequirementReason.RESTRICTED;
      denied.evidence = []; denied.resolutions = [];
      const output = m.Reply.toBinary(reply); const frame = Buffer.alloc(output.length + 5);
      frame.writeUInt32BE(output.length, 1); frame.set(output, 5);
      await route.fulfill({ response, body: Buffer.concat([frame, body.subarray(5 + length)]) });
    });
    await dialog.getByRole('button', { name: 'Add another condition', exact: true }).click();
    await dialog.getByRole('combobox', { name: 'Evidence layer' }).selectOption(String(m.RequirementLayer.OBSERVED));
    await question(dialog, m);
    await expect(dialog.getByRole('heading', { name: 'Unavailable device fact', exact: true })).toHaveCount(2);
    await expect(dialog.getByRole('region', { name: 'Selected fact editor', includeHidden: true })).toHaveCount(0);
    await expect(dialog.getByRole('button', { name: 'Resume saved input', exact: true })).toHaveCount(0);
    await expect(row(dialog, 1)).toHaveCount(2);
  });
});

for (const outcome of ['complete', 'lost']) test(`evidence pages are assembled atomically when ${outcome}`, async ({ page }) => {
  // Exercise the browser's binary transport with split real evaluator replies.
  // An incomplete second page must not publish the first page as a complete answer.
  await withFieldHost(async url => {
    const { m, dialog } = await setup(page, url, outcome === 'complete' ? 1440 : 390, 2);
    const cursors = [];
    await page.route('**/omloom.workspace.v1.Workspace/Call', async route => {
      const request = decode(m, route);
      if (request.command.oneofKind !== 'evaluateRequirements') return route.continue();
      const query = request.command.evaluateRequirements;
      const cursor = query.cursor; cursors.push(cursor);
      if (cursor && outcome === 'lost') return route.fulfill({ status: 500, body: 'Remaining evidence unavailable' });
      query.cursor = undefined;
      const input = m.Request.toBinary(request); const frame = Buffer.alloc(input.length + 5);
      frame.writeUInt32BE(input.length, 1); frame.set(input, 5);
      const response = await route.fetch({ postData: frame });
      const body = await response.body(); const length = body.readUInt32BE(1);
      const reply = m.Reply.fromBinary(body.subarray(5, 5 + length));
      const evaluation = reply.result.requirementEvaluation;
      evaluation.next = cursor ? undefined : 'next-source-page';
      for (const node of evaluation.results) for (const evidence of node.evidence) evidence.source = cursor ? 'second-page' : 'first-page';
      const output = m.Reply.toBinary(reply); const first = Buffer.alloc(output.length + 5);
      first.writeUInt32BE(output.length, 1); first.set(output, 5);
      await route.fulfill({ response, body: Buffer.concat([first, body.subarray(5 + length)]) });
    });
    await question(dialog, m, 'description', 'Ready');
    expect(cursors).toEqual([undefined, 'next-source-page']);
    if (outcome === 'complete') {
      for (const row of await dialog.locator('.fact-review-row').all()) {
        await expect(row.locator('.fact-current p')).toHaveCount(2);
        await expect(row).toContainText('first-page'); await expect(row).toContainText('second-page');
      }
    } else {
      await expect(dialog.getByRole('alert')).toBeVisible();
      await expect(dialog.locator('.fact-current p')).toHaveCount(0);
      await expect(dialog).not.toContainText('first-page');
    }
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
