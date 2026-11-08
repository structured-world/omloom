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
    const node = graph.getByRole('button', { name: '192.0.2.9, address reference', exact: true });
    await expect(node).toBeVisible();
    await expect(node).toHaveAccessibleName('192.0.2.9, address reference');
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
    await expect(page).toHaveURL(/(?:\?|&)saved=/);
    await expect(node).toHaveCSS('left', `${parseInt(left) + 16}px`);
    await page.reload();
    await expect(node).toHaveCSS('left', `${parseInt(left) + 16}px`);
    await page.getByRole('button', { name: 'Table', exact: true }).click();
    await expect(graph).toHaveCount(0);
    await expect(page.getByRole('table').getByRole('button', { name: '192.0.2.9', exact: true })).toBeVisible();
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  });
});

test('network history restores the positions of the selected view', async ({ page }) => {
  // Back and Forward restore the matching saved layout without an explicit sidebar click.
  await withFieldHost(async url => {
    const { m, rpc } = await workspaceClient(url);
    const target = (await rpc({ oneofKind: 'address', address: m.Address.create({ address: '192.0.2.12' }) })).mutation.targets[0];
    for (const [name, x] of [['Map A', 96], ['Map B', -224]]) {
      await rpc({ oneofKind: 'saveView', saveView: m.SaveView.create({
        view: { name, query: m.Query.create({ view: m.View.NETWORK, limit: 25,
          network: { layout: { pins: [{ target, x, y: 64 }] } },
        }) },
      }) });
    }
    await page.goto(`${url}/#/?view=1`);
    const graph = page.getByRole('region', { name: 'Current network topology' });
    const node = graph.getByRole('button', { name: '192.0.2.12, address reference', exact: true });
    await page.getByRole('button', { name: '◦ Map A', exact: true }).click();
    await expect(page).toHaveURL(/saved=/);
    await expect(node).toBeVisible();
    await expect(node).toHaveCSS('left', '96px');
    await page.getByRole('button', { name: '◦ Map B', exact: true }).click();
    await expect(node).toHaveCSS('left', '-224px');
    await page.goBack();
    await expect(node).toHaveCSS('left', '96px');
    await page.goForward();
    await expect(node).toHaveCSS('left', '-224px');
    await node.click();
    const detail = page.getByRole('region', { name: 'Resource details' });
    await expect(detail.getByRole('heading', { name: '192.0.2.12', exact: true })).toBeVisible();
    await detail.getByRole('button', { name: 'Close resource details' }).click();
    await graph.getByRole('button', { name: 'Arrange', exact: true }).click();
    await node.press('ArrowRight');
    await expect(node).toHaveCSS('left', '-208px');
  });
});

test('dragging the first network card preserves pointer displacement across the origin', async ({ page }) => {
  // Multiple pointer updates must not amplify a move beyond the left or top edge.
  await withFieldHost(async url => {
    const { m, rpc } = await workspaceClient(url);
    await rpc({ oneofKind: 'address', address: m.Address.create({ address: '192.0.2.13' }) });
    await page.goto(`${url}/#/?view=1`);
    const graph = page.getByRole('region', { name: 'Current network topology' });
    const node = graph.getByRole('button', { name: '192.0.2.13, address reference', exact: true });
    await graph.getByRole('button', { name: 'Arrange', exact: true }).click();
    const start = await node.evaluate(el => ({ x: parseInt(el.style.left), y: parseInt(el.style.top) }));
    const box = await node.boundingBox();
    const x = box.x + box.width / 2;
    const y = box.y + box.height / 2;
    await page.mouse.move(x, y);
    await page.mouse.down();
    for (const delta of [24, 36, 48]) {
      await page.mouse.move(x - delta, y - delta);
      await expect(node).toHaveCSS('left', `${start.x - delta}px`);
      await expect(node).toHaveCSS('top', `${start.y - delta}px`);
    }
    await page.mouse.up();
    await expect(node).toHaveCSS('left', `${start.x - 48}px`);
    await expect(node).toHaveCSS('top', `${start.y - 48}px`);
  });
});

test('an earlier network view response cannot win after a second selection', async ({ page }) => {
  // Serve the older request first, while the newer selection is still waiting.
  await withFieldHost(async url => {
    const { m, rpc } = await workspaceClient(url);
    for (const name of ['Old selection', 'New selection']) {
      await rpc({ oneofKind: 'saveView', saveView: m.SaveView.create({
        view: { name, query: m.Query.create({ view: m.View.NETWORK, search: name, limit: 25 }) },
      }) });
    }
    await page.goto(`${url}/#/?view=1`);
    const first = page.getByRole('button', { name: '◦ Old selection', exact: true });
    const second = page.getByRole('button', { name: '◦ New selection', exact: true });
    await expect(second).toBeVisible();
    const release = [Promise.withResolvers(), Promise.withResolvers()];
    const arrived = [Promise.withResolvers(), Promise.withResolvers()];
    let count = 0;
    await page.route('**/omloom.workspace.v1.Workspace/Call', async route => {
      const command = m.Request.fromBinary(route.request().postDataBuffer().subarray(5)).command;
      if (command.oneofKind !== 'views' || count === 2) return route.continue();
      const index = count++;
      arrived[index].resolve();
      await release[index].promise;
      await route.continue();
    });
    try {
      await first.click();
      await arrived[0].promise;
      await second.click();
      await arrived[1].promise;
      const returned = page.waitForResponse(response => {
        const data = response.request().postDataBuffer();
        return response.url().endsWith('/omloom.workspace.v1.Workspace/Call') && data &&
          m.Request.fromBinary(data.subarray(5)).command.oneofKind === 'views';
      });
      release[0].resolve();
      await returned;
      release[1].resolve();
      await expect(page).toHaveURL(/search=New\+selection/);
    } finally {
      for (const gate of release) gate.resolve();
    }
  });
});

test('returning to a route does not revive a pending saved view selection', async ({ page }) => {
  // Hold the complete HTTP response before delivery to the app, across a round trip in history.
  await withFieldHost(async url => {
    const { m, rpc } = await workspaceClient(url);
    await rpc({ oneofKind: 'saveView', saveView: m.SaveView.create({
      view: { name: 'Pending map', query: m.Query.create({ view: m.View.NETWORK, limit: 25 }) },
    }) });
    await page.goto(`${url}/#/?view=1`);
    const pending = page.getByRole('button', { name: '◦ Pending map', exact: true });
    await expect(pending).toBeVisible();
    await page.exposeFunction('isSavedViewsRequest', body =>
      m.Request.fromBinary(Uint8Array.from(body).subarray(5)).command.oneofKind === 'views');
    await page.evaluate(() => {
      const nativeFetch = window.fetch;
      const gate = Promise.withResolvers();
      const reply = { ready: false, consumed: false, release: gate.resolve, restore: () => { window.fetch = nativeFetch; } };
      window.heldSavedView = reply;
      window.fetch = async (input, init) => {
        const response = await nativeFetch(input, init);
        if (reply.ready || !String(input).endsWith('/omloom.workspace.v1.Workspace/Call')
            || !init?.body || !await window.isSavedViewsRequest(Array.from(init.body))) return response;
        const bytes = new Uint8Array(await response.arrayBuffer());
        reply.ready = true;
        await gate.promise;
        let sent = false;
        const body = new ReadableStream({ pull(controller) {
          if (!sent) {
            sent = true;
            controller.enqueue(bytes);
          } else {
            controller.close();
            reply.consumed = true;
          }
        } }, { highWaterMark: 0 });
        return new Response(body, { status: response.status, headers: response.headers });
      };
    });
    try {
      await pending.click();
      await expect.poll(() => page.evaluate(() => window.heldSavedView.ready)).toBe(true);
      await page.getByRole('button', { name: 'Table', exact: true }).click();
      await expect(page).toHaveURL(/networkTable=1/);
      await page.goBack();
      await expect(page).toHaveURL(/view=1$/);
      await page.evaluate(() => window.heldSavedView.release());
      await expect.poll(() => page.evaluate(() => window.heldSavedView.consumed)).toBe(true);
      await page.evaluate(() => new Promise(resolve => requestAnimationFrame(resolve)));
      await expect(page).toHaveURL(/view=1$/);
    } finally {
      await page.evaluate(() => { window.heldSavedView.release(); window.heldSavedView.restore(); });
    }
  });
});

test('network navigator expands a software product to its releases', async ({ page }) => {
  await withFieldHost(async url => {
    const { m, rpc } = await workspaceClient(url);
    const write = (kind, name) => rpc({ oneofKind: 'write', write: m.Write.create({
      kind, changes: [{ name: 'name', value: { oneofKind: 'text', text: name } }],
    }) });
    const product = (await write(m.AssetKind.SOFTWARE_PRODUCT, 'Gateway software')).mutation.targets[0];
    const release = (await write(m.AssetKind.SOFTWARE_RELEASE, 'Gateway release')).mutation.targets[0];
    await rpc({ oneofKind: 'relation', relation: m.Relationship.create({
      relation: { from: release, to: product, kind: m.RelationKind.RELEASE_OF },
    }) });
    await page.goto(`${url}/#/?view=1`);
    await page.getByText('Resource navigator', { exact: true }).click();
    const navigator = page.getByRole('navigation', { name: 'Network resources' });
    await navigator.getByRole('button', { name: 'Expand Gateway software', exact: true }).click();
    await expect(navigator.getByRole('button', { name: 'Gateway release', exact: true })).toBeVisible();
  });
});

test('selecting a network segment opens its recorded participants', async ({ page }) => {
  await withFieldHost(async url => {
    const { m, rpc } = await workspaceClient(url);
    const vm = (await rpc({ oneofKind: 'write', write: m.Write.create({
      kind: m.AssetKind.VIRTUAL_MACHINE,
      changes: [{ name: 'name', value: { oneofKind: 'text', text: 'VM without a known interface' } }],
    }) })).mutation.targets[0];
    await rpc({ oneofKind: 'segment', segment: m.Segment.create({
      name: 'Application network', participants: [{ subject: vm }],
    }) });
    await page.goto(`${url}/#/?view=1`);
    await page.getByText('Resource navigator', { exact: true }).click();
    await page.getByRole('navigation', { name: 'Network resources' })
      .getByRole('button', { name: 'Application network', exact: true }).click();
    const panel = page.getByRole('region', { name: 'L2 participation' });
    await panel.getByRole('button', { name: 'Show memberships' }).click();
    await expect(panel.getByRole('button', { name: 'VM without a known interface', exact: true })).toBeVisible();
    await expect(panel.getByText('interface unknown', { exact: false })).toBeVisible();
  });
});

for (const width of [1440, 390]) test(`network navigator opens a nested device branch at ${width}px`, async ({ page }) => {
  // Owners are navigable independently from the network kinds in the diagram.
  await withFieldHost(async url => {
    const { m, rpc } = await workspaceClient(url);
    const write = (kind, name, parent) => rpc({ oneofKind: 'write', write: m.Write.create({
      kind, parent, changes: [{ name: 'name', value: { oneofKind: 'text', text: name } }],
    }) }, parent);
    const site = (await write(m.AssetKind.SITE, 'Main site')).mutation.targets[0];
    const host = (await write(m.AssetKind.DEVICE, 'Core switch', site)).mutation.targets[0];
    await write(m.AssetKind.INTERFACE, 'Uplink', host);
    const peer = (await write(m.AssetKind.DEVICE, 'Peer switch', site)).mutation.targets[0];
    await write(m.AssetKind.INTERFACE, 'Peer uplink', peer);
    await write(m.AssetKind.L2_DOMAIN, 'Unrelated segment');
    await page.setViewportSize({ width, height: 850 });
    await page.goto(`${url}/#/?view=1`);
    const graph = page.getByRole('region', { name: 'Current network topology' });
    await expect(graph.getByRole('button', { name: 'Uplink, interface', exact: true })).toBeVisible();
    await page.getByText('Resource navigator', { exact: true }).click();
    const navigator = page.getByRole('navigation', { name: 'Network resources' });
    await navigator.getByRole('button', { name: 'Main site', exact: true }).click();
    await expect(page.locator('[aria-label="Selected network scope"]')).toContainText('Main site');
    await expect(graph.getByRole('button', { name: 'Uplink, interface', exact: true })).toBeVisible();
    await expect(graph.getByRole('button', { name: 'Peer uplink, interface', exact: true })).toBeVisible();
    await expect(graph.getByRole('button', { name: 'Unrelated segment, l2 domain', exact: true })).toHaveCount(0);
    await navigator.getByRole('button', { name: 'Expand Main site', exact: true }).click();
    await expect(navigator.getByRole('button', { name: 'Uplink', exact: true })).toHaveCount(0);
    await navigator.getByRole('button', { name: 'Expand Core switch', exact: true }).click();
    await expect(navigator.getByRole('button', { name: 'Uplink', exact: true })).toBeVisible();
    await navigator.getByRole('button', { name: 'Expand Peer switch', exact: true }).click();
    await expect(navigator.getByRole('button', { name: 'Peer uplink', exact: true })).toBeVisible();
    await expect(navigator.getByRole('button', { name: 'Uplink', exact: true })).toBeVisible();
    await navigator.getByRole('button', { name: 'Core switch', exact: true }).click();
    await expect(graph.getByRole('button', { name: 'Uplink, interface', exact: true })).toBeVisible();
    await expect(graph.getByRole('button', { name: 'Peer uplink, interface', exact: true })).toHaveCount(0);
    await expect(graph.getByRole('button', { name: 'Unrelated segment, l2 domain', exact: true })).toHaveCount(0);
    await page.getByRole('button', { name: 'Table', exact: true }).click();
    await expect(page.getByRole('table').getByRole('button', { name: 'Uplink', exact: true })).toBeVisible();
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  });
});

test('saving a resource refreshes its expanded navigator level', async ({ page }) => {
  // A rename should update the open branch without a collapse or page reload.
  await withFieldHost(async url => {
    const { m, rpc } = await workspaceClient(url);
    const write = (kind, name, parent) => rpc({ oneofKind: 'write', write: m.Write.create({
      kind, parent, changes: [{ name: 'name', value: { oneofKind: 'text', text: name } }],
    }) }, parent);
    const site = (await write(m.AssetKind.SITE, 'Main site')).mutation.targets[0];
    const host = (await write(m.AssetKind.DEVICE, 'Core switch', site)).mutation.targets[0];
    await write(m.AssetKind.INTERFACE, 'Old uplink', host);
    await page.goto(`${url}/#/?view=1`);
    await page.getByText('Resource navigator', { exact: true }).click();
    const navigator = page.getByRole('navigation', { name: 'Network resources' });
    await navigator.getByRole('button', { name: 'Expand Main site', exact: true }).click();
    await navigator.getByRole('button', { name: 'Expand Core switch', exact: true }).click();
    await page.getByRole('region', { name: 'Current network topology' })
      .getByRole('button', { name: 'Old uplink, interface', exact: true }).click();
    const detail = page.getByRole('region', { name: 'Resource details' });
    await detail.getByRole('button', { name: 'Edit resource', exact: true }).click();
    const editor = page.getByRole('dialog');
    await editor.getByLabel('Name change', { exact: true }).selectOption('set');
    await editor.getByLabel('Name', { exact: true }).fill('Renamed uplink');
    await editor.getByRole('button', { name: 'Save resource', exact: true }).click();
    await expect(detail.getByRole('heading', { name: 'Renamed uplink', exact: true })).toBeVisible();
    await expect(navigator.getByRole('button', { name: 'Renamed uplink', exact: true })).toBeVisible();
    await expect(navigator.getByRole('button', { name: 'Old uplink', exact: true })).toHaveCount(0);
  });
});

test('inventory refresh reloads site options', async ({ page }) => {
  await withFieldHost(async url => {
    const { m, rpc } = await workspaceClient(url);
    await page.goto(`${url}/#/?view=1`);
    const picker = page.getByRole('combobox', { name: 'Site', exact: true });
    await expect(picker).toBeVisible();
    await rpc({ oneofKind: 'write', write: m.Write.create({
      kind: m.AssetKind.SITE, changes: [{ name: 'name', value: { oneofKind: 'text', text: 'New site' } }],
    }) });
    await page.getByRole('button', { name: '↻ Refresh', exact: true }).click();
    await expect(picker.getByRole('option', { name: 'New site' })).toHaveCount(1);
  });
});

test('Network table can compare nested interfaces and inspect a recorded segment', async ({ page }) => {
  await withFieldHost(async url => {
    const { m, rpc } = await workspaceClient(url);
    const write = async (kind, name, parent) => (await rpc({ oneofKind: 'write', write: m.Write.create({
      kind, parent, changes: [{ name: 'name', value: { oneofKind: 'text', text: name } }],
    }) }, parent)).mutation.targets[0];
    const site = await write(m.AssetKind.SITE, 'Main site');
    const host = await write(m.AssetKind.DEVICE, 'Switch', site);
    const first = await write(m.AssetKind.INTERFACE, 'First uplink', host);
    await write(m.AssetKind.INTERFACE, 'Second uplink', host);
    const segment = await write(m.AssetKind.L2_DOMAIN, 'Recorded segment');
    await rpc({ oneofKind: 'relation', relation: m.Relationship.create({ relation: {
      from: first, to: segment, kind: m.RelationKind.ON,
    } }) });
    await page.goto(`${url}/#/?view=1&networkTable=1`);
    await page.getByText('Resource navigator', { exact: true }).click();
    await page.getByRole('navigation', { name: 'Network resources' }).getByRole('button', { name: 'Main site', exact: true }).click();
    await page.getByRole('checkbox', { name: 'Select First uplink' }).check();
    await page.getByRole('checkbox', { name: 'Select Second uplink' }).check();
    await page.getByRole('button', { name: 'Compare identity', exact: true }).click();
    const dialog = page.getByRole('dialog', { name: 'Compare identity' });
    await expect.soft(dialog.getByRole('heading', { name: 'First uplink' })).toBeVisible();
    await expect.soft(dialog.getByRole('heading', { name: 'Second uplink' })).toBeVisible();
    await dialog.getByRole('button', { name: 'Close dialog', exact: true }).click();
    await page.getByRole('table').getByRole('button', { name: 'First uplink', exact: true }).click();
    await expect(page.getByRole('region', { name: 'Resource details' }).getByRole('button', { name: 'on · Recorded segment' })).toBeVisible();
  });
});

test('history checks whether a saved map still exists', async ({ page }) => {
  await withFieldHost(async url => {
    const { m, rpc } = await workspaceClient(url);
    const target = (await rpc({ oneofKind: 'address', address: m.Address.create({ address: '192.0.2.20' }) })).mutation.targets[0];
    const view = (await rpc({ oneofKind: 'saveView', saveView: m.SaveView.create({
      view: { name: 'Removed map', query: m.Query.create({ view: m.View.NETWORK, limit: 25,
        network: { layout: { pins: [{ target, x: 128, y: 64 }] } },
      }) },
    }) })).views.items[0];
    await page.goto(`${url}/#/?view=1`);
    await page.getByRole('button', { name: '◦ Removed map', exact: true }).click();
    const node = page.getByRole('region', { name: 'Current network topology' }).getByRole('button', { name: '192.0.2.20, address reference', exact: true });
    await expect(node).toHaveCSS('left', '128px');
    await page.getByRole('button', { name: 'Hardware', exact: true }).click();
    await rpc({ oneofKind: 'saveView', saveView: m.SaveView.create({ view, remove: true }) });
    await page.goBack();
    await expect(page.getByRole('alert').filter({ hasText: 'This saved view is no longer available.' })).toBeVisible();
    await expect(node).toHaveCSS('left', '32px');
  });
});

test('a prepare failure leaves the loaded map visible', async ({ page }) => {
  await withFieldHost(async url => {
    const { m, rpc } = await workspaceClient(url);
    await rpc({ oneofKind: 'address', address: m.Address.create({ address: '192.0.2.19' }) });
    await page.route('**/omloom.workspace.v1.Workspace/Call', route => {
      const { command } = m.Request.fromBinary(route.request().postDataBuffer().subarray(5));
      return command.oneofKind === 'prepare' ? route.abort() : route.continue();
    });
    await page.goto(`${url}/#/?view=1`);
    await expect(page.getByRole('button', { name: 'Reload selection' })).toBeVisible();
    await expect(page.getByRole('region', { name: 'Current network topology' })).toBeVisible();
  });
});

test('a late site-picker failure leaves the topology usable', async ({ page }) => {
  // A separate option lookup can fail after the inventory has already rendered.
  await withFieldHost(async url => {
    const { m, rpc } = await workspaceClient(url);
    await rpc({ oneofKind: 'write', write: m.Write.create({
      kind: m.AssetKind.SITE, changes: [{ name: 'name', value: { oneofKind: 'text', text: 'Edge site' } }],
    }) });
    await rpc({ oneofKind: 'address', address: m.Address.create({ address: '192.0.2.18' }) });
    const delayed = Promise.withResolvers();
    let failSite = true;
    await page.route('**/omloom.workspace.v1.Workspace/Call', async route => {
      const command = m.Request.fromBinary(route.request().postDataBuffer().subarray(5)).command;
      if (failSite && command.oneofKind === 'query' && command.query.kinds[0] === m.AssetKind.SITE) {
        failSite = false;
        await delayed.promise;
        return route.abort();
      }
      return route.continue();
    });
    const graph = page.getByRole('region', { name: 'Current network topology' });
    const node = graph.getByRole('button', { name: '192.0.2.18, address reference', exact: true });
    try {
      await page.goto(`${url}/#/?view=1`);
      await expect(node).toBeVisible();
    } finally {
      delayed.resolve();
    }
    await expect(node).toBeVisible();
    await page.getByRole('button', { name: 'Retry sites' }).click();
    await expect(page.getByRole('combobox', { name: 'Site', exact: true }).getByRole('option', { name: 'Edge site' })).toHaveCount(1);
    await expect(node).toBeVisible();
  });
});

test('fit contains a widely arranged topology on a phone', async ({ page }) => {
  // A pinned card keeps the same canvas coordinates when another card leaves the page.
  await withFieldHost(async url => {
    const { m, rpc } = await workspaceClient(url);
    const target = (await rpc({ oneofKind: 'address', address: m.Address.create({ address: '192.0.2.22' }) })).mutation.targets[0];
    await rpc({ oneofKind: 'address', address: m.Address.create({ address: '192.0.2.1' }) });
    await rpc({ oneofKind: 'saveView', saveView: m.SaveView.create({
      view: { name: 'Wide map', query: m.Query.create({
        view: m.View.NETWORK, sort: 'name', limit: 25, network: { layout: { pins: [{ target, x: 3800, y: 64 }] } },
      }) },
    }) });
    await page.setViewportSize({ width: 390, height: 850 });
    await page.goto(`${url}/#/?view=1`);
    await page.getByRole('button', { name: '◦ Wide map', exact: true }).click();
    await expect(page).toHaveURL(/saved=/);
    const graph = page.getByRole('region', { name: 'Current network topology' });
    const node = graph.getByRole('button', { name: '192.0.2.22, address reference', exact: true });
    const scroll = graph.locator('.network-topology-scroll');
    await expect(node).toHaveCSS('left', '3800px');
    const search = page.getByRole('textbox', { name: 'Search inventory' });
    await search.fill('192.0.2.22');
    await search.press('Enter');
    await expect(graph.getByRole('button', { name: '192.0.2.1, address reference', exact: true })).toHaveCount(0);
    await expect(node).toHaveCSS('left', '3800px');
    await graph.getByRole('button', { name: 'Fit', exact: true }).click();
    expect(await scroll.evaluate(el => el.scrollWidth)).toBe(await scroll.evaluate(el => el.clientWidth));
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
    const northNode = graph.getByRole('button', { name: 'North uplink, interface', exact: true });
    const southNode = graph.getByRole('button', { name: 'South uplink, interface', exact: true });
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
    await page.unrouteAll({ behavior: 'wait' });
    await expect(northNode).toBeVisible();
    await expect(southNode).toHaveCount(0);
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
    const times = await graph.getByRole('button', { name: 'Fabric port 0, interface', exact: true }).evaluate(async node => {
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
    await graph.getByRole('button', { name: 'Fabric segment 0, l2 domain', exact: true }).click();
    const detail = page.getByRole('region', { name: 'Resource details' });
    await expect(detail.getByRole('heading', { name: 'Fabric segment 0', exact: true })).toBeVisible();
    await expect(detail.getByRole('button', { name: 'on · Fabric port 0', exact: true })).toBeVisible();
  });
});
