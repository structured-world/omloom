import { test, expect } from '@playwright/test';
import { withFieldHost } from './field-host.mjs';
import { workspaceClient } from './workspace-client.mjs';

async function setup(page, url, width = 1440) {
  page.setDefaultTimeout(5000);
  const client = await workspaceClient(url); const { m, rpc } = client;
  const target = (await rpc({ oneofKind: 'write', write: m.Write.create({ kind: m.AssetKind.DEVICE, changes: [{ name: 'name', value: { oneofKind: 'text', text: 'Measured server' } }, { name: 'description', value: { oneofKind: 'text', text: 'Preserve this description' } }] }) })).mutation.targets[0];
  await page.setViewportSize({ width, height: 900 });
  await page.goto(`${url}/#/?asset=${target.assetId}&incarnation=${target.incarnation}`);
  // Hash navigation can finish while the previous resource card is still mounted.
  await expect(page.getByRole('region', { name: 'Resource details', exact: true }).getByText(target.assetId, { exact: true })).toBeVisible();
  const panel = page.locator('.requirements-panel');
  await panel.getByText('Check recorded requirements', { exact: true }).click();
  await panel.getByRole('combobox', { name: 'Required fact', exact: true }).selectOption('drive_bays');
  await panel.getByRole('combobox', { name: 'Required comparison', exact: true }).selectOption(String(m.RequirementComparison.MINIMUM));
  await panel.getByRole('textbox', { name: 'Required value (count)', exact: true }).fill('10');
  await panel.getByRole('button', { name: 'Check requirement', exact: true }).click();
  await expect(panel.getByRole('heading', { name: 'Requirement not established', exact: true })).toBeVisible();
  return { ...client, target, panel };
}
function request(m, route) { return m.Request.fromBinary(route.request().postDataBuffer().subarray(5)); }
async function fillExact(dialog, number) {
  await dialog.getByLabel('Exact drive bays', { exact: true }).fill(number);
  await dialog.getByLabel('Evidence basis for drive bays', { exact: true }).fill('Counted physical bays');
}
for (const width of [1440, 390]) test(`one fact returns to its unchanged question at ${width}px`, async ({ page }) => {
  // Exercise real storage, focused reads, one-field writes, ranges, no-op and keyboard return.
  await withFieldHost(async url => {
    const { m, rpc, target, panel } = await setup(page, url, width);
    const writes = []; const reads = [];
    await page.route('**/omloom.workspace.v1.Workspace/Call', async route => {
      const value = request(m, route);
      if (value.command.oneofKind === 'write') writes.push(value.command.write);
      if (value.command.oneofKind === 'read' && value.command.read.acceptedField) reads.push(value.command.read);
      await route.continue();
    });
    const clarify = panel.getByRole('button', { name: 'Clarify this fact', exact: true });
    await clarify.click();
    const dialog = page.getByRole('dialog', { name: 'Clarify recorded fact', exact: true });
    await expect(dialog.getByLabel('Exact drive bays', { exact: true })).toBeFocused();
    await expect(dialog.getByLabel('Exact drive bays', { exact: true })).toHaveValue('');
    await expect(dialog.getByLabel('Maximum drive bays', { exact: true })).toHaveCount(0);
    await expect(dialog.getByLabel('Name', { exact: true })).toHaveCount(0);
    await page.keyboard.type('8'); await page.keyboard.press('Tab');
    await expect(dialog.getByLabel('Evidence basis for drive bays', { exact: true })).toBeFocused();
    await page.keyboard.type('Counted physical bays');
    if (width === 390) {
      const box = await dialog.boundingBox();
      expect(box.y).toBe(0); expect(box.width).toBe(390); expect(box.height).toBe(900);
    }
    await page.screenshot({ path: test.info().outputPath('fact-editor.png') });
    await expect(dialog.getByRole('region', { name: 'Review fact change' })).toContainText('Unknown');
    await dialog.getByRole('button', { name: 'Save and recheck', exact: true }).click();
    await expect(dialog).toBeHidden();
    const failed = panel.getByRole('heading', { name: 'Requirement not satisfied', exact: true });
    await expect(failed).toBeVisible(); await expect(failed).toBeFocused();
    await expect(panel.getByRole('textbox', { name: 'Required value (count)', exact: true })).toHaveValue('10');
    await expect(panel.getByRole('combobox', { name: 'Required comparison', exact: true })).toHaveValue(String(m.RequirementComparison.MINIMUM));
    expect(writes).toHaveLength(1); expect(writes[0].changes.map(f => f.name)).toEqual(['drive_bays']);
    expect(reads.map(r => r.acceptedField)).toEqual(['drive_bays']);
    const item = (await rpc({ oneofKind: 'read', read: m.Read.create({ target }) })).detail.item;
    expect(item.fields.find(f => f.name === 'description').value.text).toBe('Preserve this description');
    const correct = panel.getByRole('button', { name: 'Correct recorded fact', exact: true });
    await expect(correct).toBeVisible();
    await expect(panel).toContainText('A correctly recorded shortage is not fixed by changing the record.');
    await correct.click();
    await dialog.getByRole('button', { name: 'Save and recheck', exact: true }).click();
    await expect(dialog).toBeHidden(); await expect(failed).toBeVisible();
    expect(writes).toHaveLength(1);
    await correct.click();
    await dialog.getByRole('checkbox', { name: 'Only a range is known', exact: true }).check();
    await dialog.getByLabel('Maximum drive bays', { exact: true }).fill('12');
    await dialog.getByRole('button', { name: 'Save and recheck', exact: true }).click();
    await expect(dialog).toBeHidden();
    await expect(panel.getByRole('heading', { name: 'Requirement not established', exact: true })).toBeVisible();
    expect(writes).toHaveLength(2);
    await clarify.click();
    await expect(dialog.getByLabel('Minimum drive bays', { exact: true })).toHaveValue('8');
    await expect(dialog.getByLabel('Maximum drive bays', { exact: true })).toHaveValue('12');
    await dialog.getByLabel('Maximum drive bays', { exact: true }).fill('20');
    await page.keyboard.press('Escape');
    await expect(dialog).toBeHidden(); await expect(clarify).toBeFocused();
    expect(writes).toHaveLength(2);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  });
});

test('a lost Write response reads current fact without inventing a receipt or retry', async ({ page }) => {
  // The server really commits; only its response is lost at the transport boundary.
  await withFieldHost(async url => {
    const { m, rpc, target, panel } = await setup(page, url);
    let writes = 0;
    await page.route('**/omloom.workspace.v1.Workspace/Call', async route => {
      if (request(m, route).command.oneofKind !== 'write') return route.continue();
      writes++; await route.fetch(); await route.abort('connectionreset');
    });
    await panel.getByRole('button', { name: 'Clarify this fact', exact: true }).click();
    const dialog = page.getByRole('dialog', { name: 'Clarify recorded fact', exact: true });
    await fillExact(dialog, '8');
    await dialog.getByRole('button', { name: 'Save and recheck', exact: true }).click();
    await expect(dialog).toContainText('The write outcome is unknown.');
    await expect(dialog.getByRole('button', { name: 'Save and recheck', exact: true })).toBeDisabled();
    await dialog.getByRole('button', { name: 'Read and review current fact', exact: true }).click();
    await expect(dialog.getByRole('region', { name: 'Review fact change' })).toContainText('Known: 8 bays');
    await expect(dialog).toContainText('does not establish whether this attempt saved it');
    await dialog.getByRole('button', { name: 'Recheck without saving', exact: true }).click();
    await expect(dialog).toBeHidden();
    await expect(panel.getByRole('heading', { name: 'Requirement not satisfied', exact: true })).toBeVisible();
    expect(writes).toBe(1);
    const item = (await rpc({ oneofKind: 'read', read: m.Read.create({ target }) })).detail.item;
    expect(item.fields.find(f => f.name === 'drive_bays').value.equipment.assertion.quantity.lower).toBe(8n);
    await expect(panel).not.toContainText('Fact saved.');
  });
});

test('a confirmed fact survives unavailable evaluation and retries only the check', async ({ page }) => {
  // Save and evaluation are separate requests; a failed second request cannot resend the first.
  await withFieldHost(async url => {
    const { m, panel } = await setup(page, url);
    let writes = 0; let fail = true;
    await page.route('**/omloom.workspace.v1.Workspace/Call', async route => {
      const value = request(m, route);
      if (value.command.oneofKind === 'write') writes++;
      if (value.command.oneofKind === 'evaluateRequirements' && fail) return route.abort('connectionreset');
      return route.continue();
    });
    await panel.getByRole('button', { name: 'Clarify this fact', exact: true }).click();
    const dialog = page.getByRole('dialog', { name: 'Clarify recorded fact', exact: true });
    await fillExact(dialog, '8'); await dialog.getByRole('button', { name: 'Save and recheck', exact: true }).click();
    await expect(dialog).toBeHidden(); await expect(panel).toContainText('Fact saved.');
    await expect(panel.getByRole('alert')).toBeVisible();
    fail = false;
    await panel.getByRole('button', { name: 'Retry check', exact: true }).click();
    await expect(panel.getByRole('heading', { name: 'Requirement not satisfied', exact: true })).toBeVisible();
    expect(writes).toBe(1);
  });
});

test('a pending save keeps its reviewed input and cannot pretend to cancel', async ({ page }) => {
  // Delay an actual committed response, not execution, so Escape cannot undo the effect.
  await withFieldHost(async url => {
    const { m, panel } = await setup(page, url);
    let release, received;
    const pending = new Promise(resolve => { received = resolve; });
    const gate = new Promise(resolve => { release = resolve; });
    await page.route('**/omloom.workspace.v1.Workspace/Call', async route => {
      if (request(m, route).command.oneofKind !== 'write') return route.continue();
      const response = await route.fetch(); received(); await gate; await route.fulfill({ response });
    });
    try {
      await panel.getByRole('button', { name: 'Clarify this fact', exact: true }).click();
      const dialog = page.getByRole('dialog', { name: 'Clarify recorded fact', exact: true });
      await fillExact(dialog, '8'); await dialog.getByRole('button', { name: 'Save and recheck', exact: true }).click();
      await pending;
      await expect(dialog.getByLabel('Exact drive bays', { exact: true })).toBeDisabled();
      await expect(dialog.getByRole('button', { name: 'Cancel', exact: true })).toBeDisabled();
      await page.keyboard.press('Escape'); await expect(dialog).toBeVisible();
      release(); await expect(dialog).toBeHidden();
      await expect(panel.getByRole('heading', { name: 'Requirement not satisfied', exact: true })).toBeVisible();
    } finally { release(); }
  });
});

test('an unavailable current reread keeps the uncertain draft recoverable', async ({ page }) => {
  // Losing a later read does not turn a transport error into permanent permission denial.
  await withFieldHost(async url => {
    const { m, panel } = await setup(page, url);
    let failRead = false; let writes = 0;
    await page.route('**/omloom.workspace.v1.Workspace/Call', async route => {
      const command = request(m, route).command;
      if (command.oneofKind === 'write') { writes++; await route.fetch(); return route.abort('connectionreset'); }
      if (command.oneofKind === 'read' && command.read.acceptedField && failRead) return route.abort('connectionreset');
      return route.continue();
    });
    await panel.getByRole('button', { name: 'Clarify this fact', exact: true }).click();
    const dialog = page.getByRole('dialog', { name: 'Clarify recorded fact', exact: true });
    await fillExact(dialog, '8'); await dialog.getByRole('button', { name: 'Save and recheck', exact: true }).click();
    await expect(dialog).toContainText('The write outcome is unknown.');
    failRead = true;
    const reread = dialog.getByRole('button', { name: 'Read and review current fact', exact: true });
    await reread.click(); await expect(dialog.getByRole('alert')).toBeVisible();
    await expect(reread).toBeVisible();
    failRead = false; await reread.click();
    await expect(dialog.getByRole('region', { name: 'Review fact change' })).toContainText('Known: 8 bays');
    await expect(dialog.getByLabel('Exact drive bays', { exact: true })).toHaveValue('8');
    await dialog.getByRole('button', { name: 'Recheck without saving', exact: true }).click();
    await expect(panel.getByRole('heading', { name: 'Requirement not satisfied', exact: true })).toBeVisible();
    expect(writes).toBe(1);
  });
});

test('an untouched unknown text fact stays unknown instead of becoming empty text', async ({ page }) => {
  // Internal preserve state must not require a Preserve control or implicitly write an empty string.
  await withFieldHost(async url => {
    const { m, rpc, target, panel } = await setup(page, url);
    const old = (await rpc({ oneofKind: 'read', read: m.Read.create({ target }) })).detail.item;
    await rpc({ oneofKind: 'write', write: m.Write.create({ target, expected: old.revision, kind: m.AssetKind.DEVICE, changes: [{ name: 'description', value: { oneofKind: 'clear', clear: 0 } }] }) });
    await page.getByRole('button', { name: '↻ Refresh', exact: true }).click();
    await panel.getByRole('combobox', { name: 'Required fact', exact: true }).selectOption('description');
    await panel.getByRole('textbox', { name: 'Required value', exact: true }).fill('Expected description');
    await panel.getByRole('button', { name: 'Check requirement', exact: true }).click();
    await expect(panel.getByRole('heading', { name: 'Requirement not established', exact: true })).toBeVisible();
    let writes = 0;
    await page.route('**/omloom.workspace.v1.Workspace/Call', async route => { if (request(m, route).command.oneofKind === 'write') writes++; return route.continue(); });
    await panel.getByRole('button', { name: 'Clarify this fact', exact: true }).click();
    const dialog = page.getByRole('dialog', { name: 'Clarify recorded fact', exact: true });
    await expect(dialog.getByLabel('Description', { exact: true })).toBeEditable();
    await dialog.getByRole('button', { name: 'Save and recheck', exact: true }).click();
    await expect(dialog).toBeHidden();
    expect(writes).toBe(0);
    await expect(panel.getByRole('heading', { name: 'Requirement not established', exact: true })).toBeVisible();
  });
});

test('an obsolete answer cannot finish or replace a newer question', async ({ page }) => {
  // A card refresh invalidates an in-flight answer; only the new request owns busy/result state.
  await withFieldHost(async url => {
    const { m, rpc, target, panel } = await setup(page, url);
    const item = (await rpc({ oneofKind: 'read', read: m.Read.create({ target }) })).detail.item;
    await rpc({ oneofKind: 'write', write: m.Write.create({ target, expected: item.revision, kind: m.AssetKind.DEVICE, changes: [{ name: 'drive_bays', value: { oneofKind: 'integer', integer: 8n } }] }) });
    let releaseOld, releaseNew, oldReady, newReady; let evaluations = 0;
    const oldGate = new Promise(resolve => { releaseOld = resolve; });
    const newGate = new Promise(resolve => { releaseNew = resolve; });
    const oldPending = new Promise(resolve => { oldReady = resolve; });
    const newPending = new Promise(resolve => { newReady = resolve; });
    await page.route('**/omloom.workspace.v1.Workspace/Call', async route => {
      if (request(m, route).command.oneofKind !== 'evaluateRequirements') return route.continue();
      const index = ++evaluations; const response = await route.fetch();
      if (index === 1) { oldReady(); await oldGate; } else { newReady(); await newGate; }
      await route.fulfill({ response });
    });
    try {
      await panel.getByRole('button', { name: 'Check requirement', exact: true }).click(); await oldPending;
      await page.getByRole('button', { name: '↻ Refresh', exact: true }).click();
      await expect(panel.getByRole('button', { name: 'Check requirement', exact: true })).toBeEnabled();
      await panel.getByRole('combobox', { name: 'Required comparison', exact: true }).selectOption(String(m.RequirementComparison.MAXIMUM));
      await panel.getByRole('textbox', { name: 'Required value (count)', exact: true }).fill('12');
      await panel.getByRole('button', { name: 'Check requirement', exact: true }).click(); await newPending;
      const delivered = page.waitForResponse(response => {
        if (!response.url().endsWith('/omloom.workspace.v1.Workspace/Call')) return false;
        const command = m.Request.fromBinary(response.request().postDataBuffer().subarray(5)).command;
        return command.oneofKind === 'evaluateRequirements' && command.evaluateRequirements.nodes[0].expression.field.comparison === m.RequirementComparison.MINIMUM;
      });
      releaseOld(); await (await delivered).finished();
      await page.evaluate(() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve))));
      await expect(panel.getByRole('button', { name: 'Checking…', exact: true })).toBeDisabled();
      releaseNew();
      await expect(panel.getByRole('heading', { name: 'Recorded evidence matches', exact: true })).toBeVisible();
      await expect(panel.getByRole('textbox', { name: 'Required value (count)', exact: true })).toHaveValue('12');
    } finally { releaseOld(); releaseNew(); }
  });
});

test('exact quantities reject overflow and keep zero distinct from clear', async ({ page }) => {
  // Invalid input is rejected before transport, while an actual zero remains an attributed count.
  await withFieldHost(async url => {
    const { m, rpc, target, panel } = await setup(page, url);
    let writes = 0;
    await page.route('**/omloom.workspace.v1.Workspace/Call', async route => { if (request(m, route).command.oneofKind === 'write') writes++; return route.continue(); });
    await panel.getByRole('button', { name: 'Clarify this fact', exact: true }).click();
    const dialog = page.getByRole('dialog', { name: 'Clarify recorded fact', exact: true });
    await fillExact(dialog, '9223372036854775808');
    await expect(dialog.getByRole('alert')).toContainText('supported integer range');
    expect(writes).toBe(0);
    await fillExact(dialog, '0');
    await dialog.getByRole('button', { name: 'Save and recheck', exact: true }).click();
    await expect(dialog).toBeHidden();
    await expect(panel.getByRole('heading', { name: 'Requirement not satisfied', exact: true })).toBeVisible();
    const zero = (await rpc({ oneofKind: 'read', read: m.Read.create({ target, acceptedField: 'drive_bays' }) })).detail.item.fields.find(f => f.name === 'drive_bays').value.equipment.assertion.quantity;
    expect(zero.lower).toBe(0n); expect(zero.upper).toBe(0n);
    await panel.getByRole('button', { name: 'Correct recorded fact', exact: true }).click();
    await dialog.getByRole('combobox', { name: 'Knowledge for drive bays', exact: true }).selectOption('clear');
    await dialog.getByRole('button', { name: 'Save and recheck', exact: true }).click();
    await expect(dialog).toBeHidden();
    await expect(panel.getByRole('heading', { name: 'Requirement not established', exact: true })).toBeVisible();
    const cleared = (await rpc({ oneofKind: 'read', read: m.Read.create({ target, acceptedField: 'drive_bays' }) })).detail.item;
    expect(cleared.fields.find(f => f.name === 'drive_bays').value).toEqual({ oneofKind: 'clear', clear: m.NullValue.NULL_VALUE });
    expect(writes).toBe(2);
    await panel.getByRole('button', { name: 'Clarify this fact', exact: true }).click();
    await expect(dialog.getByLabel('Exact drive bays', { exact: true })).toHaveValue('');
    await dialog.getByRole('button', { name: 'Recheck without saving', exact: true }).click();
    await expect(dialog).toBeHidden(); expect(writes).toBe(2);
  });
});

for (const width of [1440, 390]) test(`a competing fact preserves the draft for explicit review at ${width}px`, async ({ page }) => {
  // A genuine intervening write must conflict, then require a fresh review without replacing user input.
  await withFieldHost(async url => {
    const { m, rpc, target, panel } = await setup(page, url, width);
    await panel.getByRole('button', { name: 'Clarify this fact', exact: true }).click();
    const dialog = page.getByRole('dialog', { name: 'Clarify recorded fact', exact: true });
    await fillExact(dialog, '8');
    const before = (await rpc({ oneofKind: 'read', read: m.Read.create({ target }) })).detail.item;
    await rpc({ oneofKind: 'write', write: m.Write.create({ target, expected: before.revision, kind: m.AssetKind.DEVICE, changes: [{ name: 'drive_bays', value: { oneofKind: 'integer', integer: 6n } }] }) });
    await dialog.getByRole('button', { name: 'Save and recheck', exact: true }).click();
    await expect(dialog.getByRole('alert')).toBeVisible();
    await expect(dialog.getByLabel('Exact drive bays', { exact: true })).toHaveValue('8');
    await expect(dialog.getByRole('button', { name: 'Save and recheck', exact: true })).toBeDisabled();
    await dialog.getByRole('button', { name: 'Read and review current fact', exact: true }).click();
    await expect(dialog.getByRole('region', { name: 'Review fact change' })).toContainText('Currently recorded drive bays: 6');
    await expect(dialog.getByLabel('Exact drive bays', { exact: true })).toHaveValue('8');
    await expect(dialog.getByLabel('Evidence basis for drive bays', { exact: true })).toHaveValue('Counted physical bays');
    await dialog.getByRole('button', { name: 'Save and recheck', exact: true }).click();
    await expect(dialog).toBeHidden();
    await expect(panel.getByRole('heading', { name: 'Requirement not satisfied', exact: true })).toBeVisible();
    const after = (await rpc({ oneofKind: 'read', read: m.Read.create({ target, acceptedField: 'drive_bays' }) })).detail.item;
    expect(after.fields.find(f => f.name === 'drive_bays').value.equipment.assertion.quantity.lower).toBe(8n);
  });
});

test('a stale incarnation never retargets its pending edit to an identically named device', async ({ page }) => {
  // Send a mismatched incarnation to the real server; labels cannot substitute another target.
  await withFieldHost(async url => {
    const { m, rpc, target, panel } = await setup(page, url);
    await panel.getByRole('button', { name: 'Clarify this fact', exact: true }).click();
    const dialog = page.getByRole('dialog', { name: 'Clarify recorded fact', exact: true });
    await fillExact(dialog, '8');
    const replacement = (await rpc({ oneofKind: 'write', write: m.Write.create({ kind: m.AssetKind.DEVICE, changes: [{ name: 'name', value: { oneofKind: 'text', text: 'Measured server' } }] }) })).mutation.targets[0];
    expect(replacement).not.toEqual(target);
    await page.route('**/omloom.workspace.v1.Workspace/Call', async route => {
      const value = request(m, route);
      const ref = value.command.oneofKind === 'write' ? value.command.write.target : value.command.oneofKind === 'read' && value.command.read.acceptedField ? value.command.read.target : undefined;
      if (!ref) return route.continue();
      ref.incarnation = replacement.incarnation;
      // The receiver's actual incarnation check, rather than a fabricated error, rejects this request.
      const bytes = m.Request.toBinary(value); const frame = Buffer.alloc(bytes.length + 5);
      frame.writeUInt32BE(bytes.length, 1); frame.set(bytes, 5);
      await route.fulfill({ response: await route.fetch({ postData: frame }) });
    });
    await dialog.getByRole('button', { name: 'Save and recheck', exact: true }).click();
    await expect(dialog.getByRole('alert')).toBeVisible();
    const reread = dialog.getByRole('button', { name: 'Read and review current fact', exact: true });
    if (await reread.isVisible()) await reread.click();
    await expect(dialog.getByRole('button', { name: 'Save and recheck', exact: true })).toBeDisabled();
    await expect(dialog.getByRole('region', { name: 'Review fact change' })).toHaveCount(0);
    const other = (await rpc({ oneofKind: 'read', read: m.Read.create({ target: replacement, acceptedField: 'drive_bays' }) })).detail.item;
    expect(other.fields.some(f => f.name === 'drive_bays')).toBe(false);
  });
});

test('denied edit admission redacts the fact and cannot issue another write', async ({ page }) => {
  // The actual server rejects a target outside the submitted working context.
  // Store tests separately exercise field-grant narrowing at the same mutation boundary.
  await withFieldHost(async url => {
    const { m, rpc, revision, target, panel } = await setup(page, url);
    await panel.getByRole('button', { name: 'Clarify this fact', exact: true }).click();
    const dialog = page.getByRole('dialog', { name: 'Clarify recorded fact', exact: true });
    await fillExact(dialog, '8');
    const other = (await rpc({ oneofKind: 'write', write: m.Write.create({ kind: m.AssetKind.DEVICE, changes: [{ name: 'name', value: { oneofKind: 'text', text: 'Another device' } }] }) })).mutation.targets[0];
    const basis = await revision(); let writes = 0;
    await page.route('**/omloom.workspace.v1.Workspace/Call', async route => {
      const value = request(m, route);
      if (value.command.oneofKind !== 'write') return route.continue();
      writes++; value.working = m.WorkingContext.create({ focus: other, expected: basis });
      const bytes = m.Request.toBinary(value); const frame = Buffer.alloc(bytes.length + 5);
      frame.writeUInt32BE(bytes.length, 1); frame.set(bytes, 5);
      await route.fulfill({ response: await route.fetch({ postData: frame }) });
    });
    await dialog.getByRole('button', { name: 'Save and recheck', exact: true }).click();
    await expect(dialog.getByRole('alert')).toBeVisible();
    await expect(dialog.getByRole('region', { name: 'Review fact change' })).toHaveCount(0);
    await expect(dialog.getByLabel('Exact drive bays', { exact: true })).toHaveCount(0);
    await expect(dialog.getByRole('button', { name: 'Save and recheck', exact: true })).toBeDisabled();
    expect(writes).toBe(1);
    const current = (await rpc({ oneofKind: 'read', read: m.Read.create({ target, acceptedField: 'drive_bays' }) })).detail.item;
    expect(current.fields.some(f => f.name === 'drive_bays')).toBe(false);
  });
});

test('a transient card reread does not discard the question after a confirmed save', async ({ page }) => {
  // A failed presentation refresh must not erase the separately reusable check or invent a failed save.
  await withFieldHost(async url => {
    const { m, panel } = await setup(page, url);
    let writes = 0; let failRead = true;
    await page.route('**/omloom.workspace.v1.Workspace/Call', async route => {
      const command = request(m, route).command;
      if (command.oneofKind === 'write') writes++;
      if (writes && failRead && command.oneofKind === 'read' && !command.read.acceptedField) return route.abort('connectionreset');
      return route.continue();
    });
    await panel.getByRole('button', { name: 'Clarify this fact', exact: true }).click();
    const dialog = page.getByRole('dialog', { name: 'Clarify recorded fact', exact: true });
    await fillExact(dialog, '8'); await dialog.getByRole('button', { name: 'Save and recheck', exact: true }).click();
    await expect(dialog).toBeHidden();
    await expect(panel.getByRole('heading', { name: 'Requirement not satisfied', exact: true })).toBeVisible();
    await expect(panel.getByRole('textbox', { name: 'Required value (count)', exact: true })).toHaveValue('10');
    await expect(page.getByText('Showing the previous resource read. Refresh before relying on these details.', { exact: true })).toBeVisible();
    failRead = false;
    await page.getByRole('button', { name: '↻ Refresh', exact: true }).click();
    await expect(panel.getByRole('textbox', { name: 'Required value (count)', exact: true })).toHaveValue('10');
    await panel.getByRole('button', { name: 'Retry check', exact: true }).click();
    await expect(panel.getByRole('heading', { name: 'Requirement not satisfied', exact: true })).toBeVisible();
    expect(writes).toBe(1);
  });
});

test('the focused journey removes field hunting and duplicate quantity entry', async ({ page }) => {
  // These are measured actions in two controlled valid journeys, not a prediction of user error rates.
  await withFieldHost(async url => {
    const generic = { preparation: 0, quantityEntries: 0, manualRechecks: 0, visibleErrors: 0 };
    const focused = { preparation: 0, quantityEntries: 0, manualRechecks: 0, visibleErrors: 0 };
    const step = async (counts, action) => { counts.preparation++; await action(); };
    let { panel } = await setup(page, url);
    await step(generic, () => page.getByRole('button', { name: 'Edit resource', exact: true }).click());
    let dialog = page.getByRole('dialog', { name: 'Edit resource', exact: true });
    await expect(dialog.getByLabel('Name', { exact: true })).toHaveCount(1);
    await expect(dialog.getByLabel('Description', { exact: true })).toHaveCount(1);
    await expect(dialog.getByLabel('Serial number', { exact: true })).toHaveCount(1);
    await step(generic, () => dialog.getByText('Equipment knowledge', { exact: true }).click());
    await step(generic, () => dialog.getByRole('combobox', { name: 'Add equipment fact', exact: true }).selectOption('drive_bays'));
    await step(generic, () => dialog.getByRole('button', { name: 'Add selected fact', exact: true }).click());
    await step(generic, () => dialog.getByRole('combobox', { name: 'Knowledge for drive bays', exact: true }).selectOption('quantity'));
    for (const name of ['Minimum drive bays', 'Maximum drive bays']) { generic.quantityEntries++; await dialog.getByLabel(name, { exact: true }).fill('8'); }
    await dialog.getByLabel('Evidence basis for drive bays', { exact: true }).fill('Counted physical bays');
    await dialog.getByRole('button', { name: 'Save resource', exact: true }).click(); await expect(dialog).toBeHidden();
    generic.manualRechecks++; await panel.getByRole('button', { name: 'Retry check', exact: true }).click();
    await expect(panel.getByRole('heading', { name: 'Requirement not satisfied', exact: true })).toBeVisible();
    generic.visibleErrors = await page.getByRole('alert').count();
    ({ panel } = await setup(page, url));
    await step(focused, () => panel.getByRole('button', { name: 'Clarify this fact', exact: true }).click());
    dialog = page.getByRole('dialog', { name: 'Clarify recorded fact', exact: true });
    await expect(dialog.getByLabel('Name', { exact: true })).toHaveCount(0);
    await expect(dialog.getByLabel('Description', { exact: true })).toHaveCount(0);
    focused.quantityEntries++; await fillExact(dialog, '8');
    await dialog.getByRole('button', { name: 'Save and recheck', exact: true }).click(); await expect(dialog).toBeHidden();
    await expect(panel.getByRole('heading', { name: 'Requirement not satisfied', exact: true })).toBeVisible();
    focused.visibleErrors = await page.getByRole('alert').count();
    expect(generic).toEqual({ preparation: 5, quantityEntries: 2, manualRechecks: 1, visibleErrors: 0 });
    expect(focused).toEqual({ preparation: 1, quantityEntries: 1, manualRechecks: 0, visibleErrors: 0 });
    console.info('Controlled fact journeys:', JSON.stringify({ generic, focused }));
  });
});

test('changing a question removes the old result before it can open a correction', async ({ page }) => {
  // A result for ten bays must not be presented or edited as an answer to twenty bays.
  await withFieldHost(async url => {
    const { panel } = await setup(page, url);
    await panel.getByRole('textbox', { name: 'Required value (count)', exact: true }).fill('20');
    await expect(panel.getByRole('region', { name: 'Requirement result', exact: true })).toHaveCount(0);
    await expect(panel.getByRole('button', { name: 'Clarify this fact', exact: true })).toHaveCount(0);
    await panel.getByRole('button', { name: 'Check requirement', exact: true }).click();
    await panel.getByRole('button', { name: 'Clarify this fact', exact: true }).click();
    const dialog = page.getByRole('dialog', { name: 'Clarify recorded fact', exact: true });
    await expect(dialog).toContainText('drive bays at least 20 count');
    await expect(dialog.getByLabel('Exact drive bays', { exact: true })).toHaveValue('');
    await fillExact(dialog, '8'); await dialog.getByRole('button', { name: 'Save and recheck', exact: true }).click();
    await expect(panel.getByRole('heading', { name: 'Requirement not satisfied', exact: true })).toBeVisible();
    await expect(panel).toContainText('drive bays at least 20 count');
  });
});
