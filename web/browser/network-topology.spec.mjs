import { test, expect } from '@playwright/test';
import { withFieldHost } from './field-host.mjs';
import { workspaceClient } from './workspace-client.mjs';

for (const width of [1440, 390]) test(`known topology supports inspect, arrange and table at ${width}px`, async ({ page }) => {
  // The diagram reads recorded links, and local presentation edits only write on explicit save.
  await withFieldHost(async url => {
    const { m, rpc, revision } = await workspaceClient(url);
    await rpc({ oneofKind: 'address', address: m.Address.create({ address: '192.0.2.9/24', createPrefix: true }) });
    const before = await revision();
    await page.setViewportSize({ width, height: 900 });
    await page.goto(`${url}/#/?view=1`);
    const graph = page.getByRole('region', { name: 'Current network topology' });
    const node = graph.getByRole('button', { name: '192.0.2.9', exact: true });
    await expect(node).toBeVisible();
    await expect(graph.locator('svg path')).toHaveCount(1);
    await graph.getByRole('button', { name: 'Fit', exact: true }).click();
    await node.focus();
    await node.press('Enter');
    const detail = page.getByRole('region', { name: 'Resource details' });
    await expect(detail.getByRole('heading', { name: '192.0.2.9', exact: true })).toBeFocused();
    await detail.getByRole('button', { name: 'Close resource details' }).click();
    await expect(node).toBeFocused();
    await graph.getByRole('button', { name: 'Arrange', exact: true }).click();
    const left = await node.evaluate(el => el.style.left);
    await node.press('ArrowRight');
    await expect.poll(() => node.evaluate(el => el.style.left)).not.toBe(left);
    expect(await revision()).toBe(before);
    await graph.getByRole('button', { name: 'Done arranging' }).click();
    await page.getByRole('button', { name: 'Save view', exact: true }).click();
    const dialog = page.getByRole('dialog', { name: 'Save workspace view' });
    await dialog.getByLabel('Name', { exact: true }).fill('Arranged network');
    await dialog.getByRole('button', { name: 'Save personal view' }).click();
    await expect(dialog).toBeHidden();
    await page.reload();
    await page.getByRole('button', { name: '◦ Arranged network', exact: true }).click();
    await expect(node).toBeVisible();
    expect(parseInt(await node.evaluate(el => el.style.left))).toBe(parseInt(left) + 16);
    await page.getByRole('button', { name: 'Table', exact: true }).click();
    await expect(graph).toHaveCount(0);
    await expect(page.getByRole('table').getByRole('button', { name: '192.0.2.9', exact: true })).toBeVisible();
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  });
});

test('site and project intersect and obsolete scope requests cannot restore nodes', async ({ page }) => {
  // The selected saved roots are independent of location, even on an empty intersection.
  await withFieldHost(async url => {
    const { m, rpc } = await workspaceClient(url);
    async function create(kind, name, parent) {
      const result = await rpc({ oneofKind: 'write', write: m.Write.create({
        kind, parent, changes: [{ name: 'name', value: { oneofKind: 'text', text: name } }],
      }) }, parent);
      return result.mutation.targets[0];
    }
    const north = await create(m.AssetKind.SITE, 'North site');
    const south = await create(m.AssetKind.SITE, 'South site');
    const northHost = await create(m.AssetKind.DEVICE, 'North host', north);
    const southHost = await create(m.AssetKind.DEVICE, 'South host', south);
    await create(m.AssetKind.INTERFACE, 'North uplink', northHost);
    await create(m.AssetKind.INTERFACE, 'South uplink', southHost);
    const project = (await rpc({ oneofKind: 'saveView', saveView: m.SaveView.create({
      view: { name: 'North project', query: m.Query.create({ view: m.View.NETWORK, roots: [northHost], limit: 25 }) },
    }) })).views.items[0];
    await page.setViewportSize({ width: 390, height: 850 });
    await page.goto(`${url}/#/?view=1`);
    const graph = page.getByRole('region', { name: 'Current network topology' });
    const site = page.getByRole('combobox', { name: 'Site', exact: true });
    const projectPicker = page.getByRole('combobox', { name: 'Project / selection', exact: true });
    const northNode = graph.getByRole('button', { name: 'North uplink', exact: true });
    const southNode = graph.getByRole('button', { name: 'South uplink', exact: true });
    await projectPicker.selectOption(project.id);
    await expect(northNode).toBeVisible();

    let release;
    const sendSouth = new Promise(resolve => { release = resolve; });
    const isSouth = request => {
      if (!request.url().endsWith('/omloom.workspace.v1.Workspace/Call')) return false;
      const command = m.Request.fromBinary(request.postDataBuffer().subarray(5)).command;
      return command.oneofKind === 'query' && command.query.network?.sites[0]?.assetId === south.assetId;
    };
    await page.route('**/omloom.workspace.v1.Workspace/Call', async route => {
      if (!isSouth(route.request())) return route.continue();
      const pending = route.fetch();
      await sendSouth;
      try { await route.fulfill({ response: await pending }); } catch (error) {
        if (!String(error).includes('Request is already handled')) throw error;
      }
    });
    const waitForSouth = page.waitForRequest(isSouth);
    try {
      await site.selectOption({ label: 'South site' });
      await waitForSouth;
      await site.selectOption({ label: 'All permitted locations' });
      await expect(northNode).toBeVisible();
    } finally {
      release();
    }
    await expect(southNode).toHaveCount(0);
    await page.unrouteAll({ behavior: 'ignoreErrors' });
    await site.selectOption({ label: 'South site' });
    await expect(page.getByRole('heading', { name: 'No resources match this selection' })).toBeVisible();
    await expect(southNode).toHaveCount(0);
    await projectPicker.selectOption('');
    await expect(southNode).toBeVisible();
    await expect(northNode).toHaveCount(0);
  });
});

test('dense topology bounds browser work and keeps the neighborhood inspectable', async ({ page }) => {
  // A bipartite cyclic graph with 25 nodes and 144 recorded links exceeds the edge budget.
  await withFieldHost(async url => {
    const { m, rpc } = await workspaceClient(url);
    async function create(kind, name, parent) {
      const response = await rpc({ oneofKind: 'write', write: m.Write.create({
        kind, parent, changes: [{ name: 'name', value: { oneofKind: 'text', text: name } }],
      }) }, parent);
      return response.mutation.targets[0];
    }
    const host = await create(m.AssetKind.DEVICE, 'Edge host');
    const interfaces = [];
    for (let index = 0; index < 16; index++) interfaces.push(await create(m.AssetKind.INTERFACE, `Fabric port ${index}`, host));
    for (let index = 0; index < 9; index++) {
      const segment = await create(m.AssetKind.L2_DOMAIN, `Fabric segment ${index}`);
      for (const from of interfaces) await rpc({ oneofKind: 'relation', relation: m.Relationship.create({
        relation: { from, to: segment, kind: m.RelationKind.ON },
      }) });
    }
    await page.setViewportSize({ width: 1440, height: 900 });
    const cdp = await page.context().newCDPSession(page);
    await page.goto(`${url}/#/?view=1&search=Fabric`);
    const graph = page.getByRole('region', { name: 'Current network topology' });
    await expect(graph.locator('.network-topology-node')).toHaveCount(25);
    await expect(graph.locator('svg path')).toHaveCount(128);
    await expect(graph.getByText('More recorded links are available in resource details.')).toBeVisible();
    await cdp.send('HeapProfiler.collectGarbage');
    const heap = await cdp.send('Runtime.getHeapUsage');
    await graph.getByRole('button', { name: 'Arrange', exact: true }).click();
    const times = await graph.getByRole('button', { name: 'Fabric port 0', exact: true }).evaluate(async node => {
      const times = [];
      for (let index = 0; index < 50; index++) {
        const start = performance.now();
        node.dispatchEvent(new KeyboardEvent('keydown', { key: index % 2 ? 'ArrowLeft' : 'ArrowRight', bubbles: true }));
        await new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)));
        times.push(performance.now() - start);
      }
      return times.sort((a, b) => a - b);
    });
    console.log(`Dense Chromium topology: whole-page JS heap ${heap.usedSize} bytes, keyboard-to-render p95 ${times[47]} ms, p99 ${times[49]} ms`);
    await graph.getByRole('button', { name: 'Done arranging' }).click();
    await graph.getByRole('button', { name: 'Fabric segment 0', exact: true }).click();
    const detail = page.getByRole('region', { name: 'Resource details' });
    await expect(detail.getByRole('heading', { name: 'Fabric segment 0', exact: true })).toBeVisible();
    await expect(detail.getByRole('button', { name: 'on · Fabric port 0', exact: true })).toBeVisible();
  });
});
