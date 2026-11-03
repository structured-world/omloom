import { test, expect } from '@playwright/test';
import { withFieldHost } from './field-host.mjs';
import { workspaceClient } from './workspace-client.mjs';

for (const width of [1440, 390]) test(`segment names validate UTF-8 bytes before preview at ${width}px`, async ({ page }) => {
  // Unicode labels at the byte limit remain valid; longer drafts must not reach the server.
  await withFieldHost(async url => {
    const { m, rpc } = await workspaceClient(url);
    const vm = (await rpc({ oneofKind: 'write', write: m.Write.create({ kind: m.AssetKind.VIRTUAL_MACHINE, changes: [{ name: 'name', value: { oneofKind: 'text', text: 'Unicode VM' } }] }) })).mutation.targets[0];
    await page.setViewportSize({ width, height: 900 });
    await page.goto(`${url}/#/?asset=${vm.assetId}&incarnation=${vm.incarnation}`);
    await page.getByRole('button', { name: 'Record shared segment', exact: true }).click();
    const dialog = page.getByRole('dialog', { name: 'Shared L2 segment', exact: true });
    let previews = 0;
    page.on('request', request => {
      if (request.url().endsWith('/omloom.workspace.v1.Workspace/Call') && m.Request.fromBinary(request.postDataBuffer().subarray(5)).command.oneofKind === 'previewSegment') previews++;
    });
    const name = dialog.getByLabel('Name (optional)', { exact: true });
    await name.fill('é'.repeat(81));
    await dialog.getByRole('button', { name: 'Review participation', exact: true }).click();
    await expect(dialog.getByRole('alert')).toContainText('160 UTF-8 bytes');
    expect(previews).toBe(0);
    await expect(name).toHaveValue('é'.repeat(81));
    await expect(dialog.getByRole('region', { name: 'Membership summary' })).toHaveCount(0);
    await name.fill('é'.repeat(80));
    await dialog.getByRole('button', { name: 'Review participation', exact: true }).click();
    await expect(dialog.getByRole('region', { name: 'Membership summary' })).toContainText('é'.repeat(80));
    expect(previews).toBe(1);
    await dialog.getByRole('button', { name: 'Save reviewed participation', exact: true }).click();
    await expect(dialog).toBeHidden();
    const segments = (await rpc({ oneofKind: 'query', query: m.Query.create({ kinds: [m.AssetKind.L2_DOMAIN], columns: ['name'], limit: 1 }) })).page;
    expect(segments.total).toBe(1n);
    expect(segments.items[0].fields).toEqual([{ name: 'name', value: { oneofKind: 'text', text: 'é'.repeat(80) } }]);
  });
});

for (const width of [1440, 390]) test(`segment revision conflict preserves an editable draft at ${width}px`, async ({ page }) => {
  // A confirmed rejection permits a fresh review without losing the selected VM or interface.
  await withFieldHost(async url => {
    const { m, rpc } = await workspaceClient(url);
    const create = async (kind, name, parent) => (await rpc({ oneofKind: 'write', write: m.Write.create({ kind, parent, changes: [{ name: 'name', value: { oneofKind: 'text', text: name } }] }) }, parent)).mutation.targets[0];
    const vm = await create(m.AssetKind.VIRTUAL_MACHINE, 'Conflict VM');
    const nic = await create(m.AssetKind.INTERFACE, 'eth-conflict', vm);
    await page.setViewportSize({ width, height: 900 });
    await page.goto(`${url}/#/?asset=${vm.assetId}&incarnation=${vm.incarnation}`);
    await page.getByRole('button', { name: 'Record shared segment', exact: true }).click();
    const dialog = page.getByRole('dialog', { name: 'Shared L2 segment', exact: true });
    await dialog.getByLabel('Name (optional)', { exact: true }).fill('Conflict draft');
    await dialog.getByRole('button', { name: 'Find interfaces of this VM', exact: true }).click();
    await dialog.getByRole('button', { name: 'Use interface', exact: true }).click();
    await dialog.getByRole('button', { name: 'Review participation', exact: true }).click();
    await expect(dialog.getByRole('region', { name: 'Membership summary' })).toContainText('eth-conflict');
    await create(m.AssetKind.DEVICE, 'Independent change after review');
    await dialog.getByRole('button', { name: 'Save reviewed participation', exact: true }).click();
    await expect(dialog.getByRole('button', { name: 'Review participation', exact: true })).toBeVisible();
    await expect(dialog.getByRole('alert')).toContainText('State changed');
    await expect(dialog.getByLabel('Name (optional)', { exact: true })).toHaveValue('Conflict draft');
    await expect(dialog.getByRole('group', { name: 'Conflict VM', exact: true })).toContainText('Interface: eth-conflict');
    await expect(dialog.getByRole('button', { name: 'Retry reviewed command', exact: true })).toHaveCount(0);
    expect((await rpc({ oneofKind: 'query', query: m.Query.create({ kinds: [m.AssetKind.L2_DOMAIN], limit: 1 }) })).page.total).toBe(0n);
    await dialog.getByLabel('Name (optional)', { exact: true }).fill('Reviewed again');
    await dialog.getByRole('button', { name: 'Review participation', exact: true }).click();
    await dialog.getByRole('button', { name: 'Save reviewed participation', exact: true }).click();
    await expect(dialog).toBeHidden();
    const result = (await rpc({ oneofKind: 'segmentMemberships', segmentMemberships: m.SegmentMemberships.create({ targets: [vm] }) })).segmentMembershipPage;
    expect(result.entries).toHaveLength(1);
    expect(result.entries[0].source.target).toEqual(nic);
    expect(result.entries[0].segment.fields).toEqual([{ name: 'name', value: { oneofKind: 'text', text: 'Reviewed again' } }]);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  });
});

for (const width of [1440, 390]) test(`selected VMs share and refine a segment at ${width}px`, async ({ page }) => {
  // Real disk-backed commands and browser actions preserve unknown interfaces,
  // relation identity and other participation after a lost mutation response.
  await withFieldHost(async url => {
    const { m, rpc } = await workspaceClient(url);
    const create = async (kind, name, parent) => (await rpc({ oneofKind: 'write', write: m.Write.create({ kind, parent, changes: [{ name: 'name', value: { oneofKind: 'text', text: name } }] }) }, parent)).mutation.targets[0];
    const first = await create(m.AssetKind.VIRTUAL_MACHINE, 'Segment VM one');
    const second = await create(m.AssetKind.VIRTUAL_MACHINE, 'Segment VM two');
    const nic = await create(m.AssetKind.INTERFACE, 'eth0', first);
    await page.setViewportSize({ width, height: 900 });
    await page.goto(`${url}/#/?view=3`);
    await page.getByRole('checkbox', { name: 'Select Segment VM one', exact: true }).check();
    await page.getByRole('checkbox', { name: 'Select Segment VM two', exact: true }).check();
    await page.getByRole('button', { name: 'Record shared segment', exact: true }).click();
    const dialog = page.getByRole('dialog', { name: 'Shared L2 segment', exact: true });
    await expect(dialog.getByRole('group', { name: 'Segment VM one', exact: true })).toContainText('Interface: Unknown');
    await dialog.getByLabel('Name (optional)', { exact: true }).fill('Draft segment');
    await dialog.getByRole('button', { name: 'Review participation', exact: true }).click();
    await dialog.getByRole('button', { name: 'Back', exact: true }).click();
    await expect(dialog).toBeVisible();
    await expect(dialog.getByLabel('Name (optional)', { exact: true })).toHaveValue('Draft segment');
    await dialog.getByLabel('Name (optional)', { exact: true }).fill('');
    await dialog.getByRole('button', { name: 'Review participation', exact: true }).click();
    await expect(dialog.getByRole('region', { name: 'Membership summary' })).toContainText('New unnamed L2 segment');
    expect((await rpc({ oneofKind: 'query', query: m.Query.create({ kinds: [m.AssetKind.L2_DOMAIN], limit: 1 }) })).page.total).toBe(0n);
    let commands = 0;
    await page.route('**/omloom.workspace.v1.Workspace/Call', async route => {
      const request = m.Request.fromBinary(route.request().postDataBuffer().subarray(5));
      if (request.command.oneofKind === 'segment') {
        commands++;
        if (commands === 1) { await route.fetch(); await route.abort('failed'); return; }
      }
      await route.continue();
    });
    await dialog.getByRole('button', { name: 'Save reviewed participation', exact: true }).click();
    await expect(dialog.getByRole('alert')).toContainText('reviewed command is retained');
    await dialog.getByRole('button', { name: 'Retry reviewed command', exact: true }).click();
    await expect(dialog).toBeHidden();
    expect(commands).toBe(2);
    const segments = (await rpc({ oneofKind: 'query', query: m.Query.create({ kinds: [m.AssetKind.L2_DOMAIN], columns: ['name'], limit: 25 }) })).page;
    expect(segments.total).toBe(1n);
    expect(segments.items[0].fields).toEqual([]);
    const before = (await rpc({ oneofKind: 'segmentMemberships', segmentMemberships: m.SegmentMemberships.create({ targets: [first, second] }) })).segmentMembershipPage;
    expect(before.entries).toHaveLength(2);
    expect(before.entries.every(e => e.source.kind === m.AssetKind.VIRTUAL_MACHINE)).toBe(true);
    const assertion = before.entries.find(e => e.source.target.assetId === first.assetId).relation.relationId;
    await page.goto(`${url}/#/?scope=${first.assetId}&scopeInc=${first.incarnation}&asset=${first.assetId}&incarnation=${first.incarnation}`);
    const panel = page.getByRole('region', { name: 'L2 participation', exact: true });
    await panel.getByRole('button', { name: 'Show memberships', exact: true }).click();
    await expect(panel.getByRole('list', { name: 'Selected network projection' })).toContainText('interface unknown');
    await panel.getByRole('button', { name: 'Refine participation', exact: true }).click();
    const participant = dialog.getByRole('group', { name: 'Segment VM one', exact: true });
    await participant.getByRole('button', { name: 'Find interfaces of this VM', exact: true }).click();
    await participant.getByRole('button', { name: 'Use interface', exact: true }).click();
    await dialog.getByRole('button', { name: 'Review participation', exact: true }).click();
    await expect(dialog.getByRole('region', { name: 'Membership summary' })).toContainText('eth0');
    await expect(dialog.getByRole('region', { name: 'Membership summary' })).toContainText(assertion.slice(-8));
    await dialog.getByRole('button', { name: 'Save reviewed participation', exact: true }).click();
    await expect(dialog).toBeHidden();
    const after = (await rpc({ oneofKind: 'segmentMemberships', segmentMemberships: m.SegmentMemberships.create({ targets: [first, second] }) })).segmentMembershipPage;
    expect(after.entries).toHaveLength(2);
    const refined = after.entries.find(e => e.relation.relationId === assertion);
    expect(refined.source.target).toEqual(nic);
    expect(refined.subject.target).toEqual(first);
    expect(after.entries.find(e => e.source.target.assetId === second.assetId).relation).toEqual(before.entries.find(e => e.source.target.assetId === second.assetId).relation);
    await panel.getByRole('button', { name: 'Show memberships', exact: true }).click();
    await expect(panel.getByRole('list', { name: 'Selected network projection' })).toContainText('eth0');
    await panel.getByRole('button', { name: new RegExp(segments.items[0].target.assetId.slice(-8)) }).click();
    const detail = page.getByRole('region', { name: 'Resource details', exact: true });
    await expect(detail.getByText(segments.items[0].target.assetId, { exact: true })).toBeVisible();
    await panel.getByRole('button', { name: 'Show memberships', exact: true }).click();
    await expect(panel.getByRole('list', { name: 'Selected network projection' })).toContainText('Segment VM one');
    await expect(panel.getByRole('list', { name: 'Selected network projection' })).toContainText('eth0');
    await page.getByRole('button', { name: 'Close resource details', exact: true }).click();
    await expect(detail.getByText(first.assetId, { exact: true })).toBeVisible();
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    await page.screenshot({ path: `test-results/segments-${width}.png`, fullPage: true });
  });
});

for (const width of [1440,390]) test(`CSV segment import and reviewed merge at ${width}px`,async({page})=>{
  // The import form, correction preview and common reversal operate on the same actual disk state.
  await withFieldHost(async url=>{
    page.setDefaultTimeout(5000);
    const {m,rpc}=await workspaceClient(url);
    const vm=(await rpc({oneofKind:'write',write:m.Write.create({kind:m.AssetKind.VIRTUAL_MACHINE,changes:[{name:'name',value:{oneofKind:'text',text:'Import VM'}}]})})).mutation.targets[0];
    await page.setViewportSize({width,height:900});await page.goto(url);
    if (width < 700) await page.getByRole('button',{name:'More',exact:true}).click();
    await page.getByRole('button',{name:'Import CSV',exact:true}).click();
    let dialog=page.getByRole('dialog',{name:'Import inventory',exact:true});
    await dialog.getByLabel('Source name',{exact:true}).fill('Selected segments');
    await dialog.getByRole('combobox',{name:'New resource kind',exact:true}).selectOption(String(m.AssetKind.L2_DOMAIN));
    await dialog.getByLabel('CSV',{exact:true}).fill(`external_id,name,participants\nsegment-1,,${vm.assetId}/${vm.incarnation}\n`);
    await dialog.getByRole('button',{name:'Add mapping',exact:true}).click();
    await dialog.getByLabel('CSV column',{exact:true}).nth(1).fill('participants');
    expect(await dialog.getByRole('combobox',{name:'Inventory field',exact:true}).nth(1).locator('option').evaluateAll(options=>options.map(option=>option.value))).toContain('participants');
    await dialog.getByRole('combobox',{name:'Inventory field',exact:true}).nth(1).selectOption('participants');
    await dialog.getByRole('button',{name:'Prepare preview',exact:true}).click();
    await expect(dialog.getByRole('button',{name:'Accept reviewed rows',exact:true})).toBeVisible();
    await dialog.getByRole('button',{name:'Accept reviewed rows',exact:true}).click();await expect(dialog).toBeHidden();
    const imported=(await rpc({oneofKind:'query',query:m.Query.create({kinds:[m.AssetKind.L2_DOMAIN],limit:25,columns:['name']})})).page.items[0].target;
    const kept=(await rpc({oneofKind:'write',write:m.Write.create({kind:m.AssetKind.L2_DOMAIN,changes:[{name:'name',value:{oneofKind:'text',text:'Retained segment'}}]})})).mutation.targets[0];
    await rpc({oneofKind:'segment',segment:m.Segment.create({target:kept,participants:[{subject:vm}]})});
    await page.goto(`${url}/#/?view=1`);
    await page.getByRole('checkbox',{name:new RegExp(`^Select l2 domain.*${imported.assetId.slice(-8)}`)}).check();
    await page.getByRole('checkbox',{name:'Select Retained segment',exact:true}).check();
    await page.getByRole('button',{name:'Compare identity',exact:true}).click();
    dialog=page.getByRole('dialog',{name:'Compare identity',exact:true});
    await dialog.getByRole('button',{name:'Plan identity correction',exact:true}).click();
    await dialog.getByRole('radio',{name:'Duplicate records of the same L2 segment',exact:true}).check();
    await dialog.getByRole('button',{name:'Continue',exact:true}).click();
    await dialog.getByRole('combobox',{name:'Retained identity',exact:true}).selectOption({label:'Retained segment'});
    for(const checkbox of await dialog.getByRole('checkbox').all()) await checkbox.check();
    await dialog.getByRole('button',{name:'Prepare correction preview',exact:true}).click();
    await expect(dialog.getByRole('heading',{name:'Review identity correction',exact:true})).toBeVisible();
    await dialog.getByRole('button',{name:'Accept identity correction',exact:true}).click();
    await expect(dialog.getByRole('heading',{name:'Correction applied',exact:true})).toBeVisible();
    const merged=(await rpc({oneofKind:'segmentMemberships',segmentMemberships:m.SegmentMemberships.create({targets:[vm]})})).segmentMembershipPage;
    expect(merged.entries).toHaveLength(1);expect(merged.entries[0].segment.target).toEqual(kept);
    await dialog.getByRole('button',{name:'Reverse this correction',exact:true}).click();
    await expect(dialog.getByRole('heading',{name:'Correction reversed',exact:true})).toBeVisible();
    expect((await rpc({oneofKind:'segmentMemberships',segmentMemberships:m.SegmentMemberships.create({targets:[vm]})})).segmentMembershipPage.entries).toHaveLength(2);
    expect(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth)).toBe(true);
  });
});
