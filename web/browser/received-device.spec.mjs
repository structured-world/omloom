import {test,expect} from '@playwright/test';
import {withFieldHost} from './field-host.mjs';
import {workspaceClient} from './workspace-client.mjs';

async function fixture(url){
  const {m,rpc}=await workspaceClient(url);
  const rack=(await rpc({oneofKind:'write',write:m.Write.create({kind:m.AssetKind.RACK,changes:[{name:'name',value:{oneofKind:'text',text:'Receiving rack'}}]})})).mutation.targets[0];
  const plan=(await rpc({oneofKind:'physical',physical:m.Physical.create({action:{oneofKind:'save',save:{plan:{name:'Unpack three nodes',materials:[{name:'Ordered compute node',quantity:3,kind:m.AssetKind.DEVICE}]}}}})})).physical.plan;
  return {m,rpc,rack,plan};
}
async function openPlan(page,url,f){
  await page.goto(`${url}/#/?view=2&asset=${f.rack.assetId}&incarnation=${f.rack.incarnation}&scope=${f.rack.assetId}&scopeInc=${f.rack.incarnation}`);
  await page.getByText('Plan installation from enclosing scope',{exact:true}).click();
  const planner=page.locator('.physical-planner');
  await planner.getByRole('button',{name:'Unpack three nodes · draft',exact:true}).click();
  await planner.getByText('Required materials and receiving (1 lines)',{exact:true}).click();
  await planner.getByRole('button',{name:'Receive or reuse a unit for requirement 1',exact:true}).click();
  return planner;
}

for(const width of [1440,390])test(`received devices remain unplaced and record-next clears serial at ${width}px`,async({page})=>{
  // The existing inventory editor creates actual fields and allocation in one commit,
  // without a create/search detour, copied serial, or fabricated rack attachment.
  await withFieldHost(async url=>{
    const f=await fixture(url);await page.setViewportSize({width,height:900});
    const planner=await openPlan(page,url,f);
    await planner.getByLabel('Requested unit number',{exact:true}).fill('2');
    await planner.getByRole('button',{name:'Create received device',exact:true}).click();
    let editor=planner.getByRole('region',{name:'Create received device',exact:true});
    await expect(editor).toContainText('Unit 2');
    await expect(editor.getByLabel('Name',{exact:true})).toHaveValue('Ordered compute node');
    await editor.getByLabel('Name',{exact:true}).fill('Unpacked node two');
    await editor.getByLabel('Serial number change',{exact:true}).selectOption('set');
    await editor.getByLabel('Serial number',{exact:true}).fill('SERIAL-TWO');
    await editor.getByRole('button',{name:'Record and next',exact:true}).click();
    await expect(planner).toContainText('1 allocated of 3 requested');
    editor=planner.getByRole('region',{name:'Create received device',exact:true});
    await expect(editor).toContainText('Unit 1');
    await expect(editor.getByLabel('Serial number',{exact:true})).toHaveValue('');
    await expect(editor.getByLabel('Serial number change',{exact:true})).toHaveValue('preserve');
    await editor.getByLabel('Name',{exact:true}).fill('Unpacked node one');
    await expect(editor.getByLabel('Name',{exact:true})).toHaveValue('Unpacked node one');
    await editor.getByRole('button',{name:'Record received device',exact:true}).click();
    await expect(planner).toContainText('2 allocated of 3 requested');
    await expect(editor.getByRole('status')).toContainText('Receipt accepted');
    const loaded=(await f.rpc({oneofKind:'physical',physical:f.m.Physical.create({action:{oneofKind:'load',load:{id:f.plan.id}}})})).physical.plan;
    expect(loaded.materials[0].allocations.map(a=>a.unit)).toEqual([0,1]);
    for(const allocation of loaded.materials[0].allocations){
      const detail=(await f.rpc({oneofKind:'read',read:f.m.Read.create({target:allocation.item})})).detail;
      expect(detail.relations).toEqual([]);
      expect(detail.item.fields.find(v=>v.name==='width')).toBeUndefined();
      if(allocation.unit===0)expect(detail.item.fields.find(v=>v.name==='serial')).toBeUndefined();
    }
    await page.screenshot({path:`test-results/received-device-${width}.png`,fullPage:true});
    await page.reload();
    const reloaded=await openPlan(page,url,f);
    await expect(reloaded).toContainText('Unit 2: Unpacked node two');
    await expect(reloaded).toContainText('Unit 1: Unpacked node one');
    expect(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth)).toBe(true);
  });
});

test('a lost receipt reply is recovered after reload without another device',async({page})=>{
  // The browser retains only a lookup reference; a lost acknowledged response never
  // changes into fresh creation, even after the editor has been reopened.
  await withFieldHost(async url=>{
    const f=await fixture(url);await page.setViewportSize({width:390,height:900});
    const planner=await openPlan(page,url,f);
    await planner.getByRole('button',{name:'Create received device',exact:true}).click();
    const editor=planner.getByRole('region',{name:'Create received device',exact:true});
    await editor.getByLabel('Name',{exact:true}).fill('Reply lost node');
    let lossFinished;
    const loss=new Promise(resolve=>{lossFinished=resolve;});
    const lose=async route=>{
      const request=f.m.Request.fromBinary(route.request().postDataBuffer().subarray(5));
      if(request.command.oneofKind==='physical'&&request.command.physical.action.oneofKind==='receiveDevice'){
        await route.fetch();await route.abort('failed');lossFinished();
      }else await route.continue();
    };
    await page.route('**/omloom.workspace.v1.Workspace/Call',lose);
    await editor.getByRole('button',{name:'Record received device',exact:true}).click();
    await expect(editor.getByRole('button',{name:'Recover receipt result',exact:true})).toBeVisible();
    await loss;
    const reference=await page.evaluate(()=>JSON.parse(localStorage.getItem('omloom.pending-device-receipt')));
    expect(Object.keys(reference).sort()).toEqual(['plan','revision']);
    await page.unroute('**/omloom.workspace.v1.Workspace/Call',lose);
    await page.reload();const reopened=await openPlan(page,url,f);
    await reopened.getByRole('button',{name:'Create received device',exact:true}).click();
    await reopened.getByRole('button',{name:'Recover receipt result',exact:true}).click();
    await expect(reopened).toContainText('Receipt accepted');
    const devices=(await f.rpc({oneofKind:'query',query:f.m.Query.create({kinds:[f.m.AssetKind.DEVICE],columns:['name'],limit:25})})).page;
    expect(devices.total).toBe(1n);
    expect(devices.items[0].fields).toEqual([{name:'name',value:{oneofKind:'text',text:'Reply lost node'}}]);
  });
});
