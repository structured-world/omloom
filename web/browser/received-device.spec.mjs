import {test,expect} from '@playwright/test';
import {withFieldHost} from './field-host.mjs';
import {workspaceClient} from './workspace-client.mjs';

async function fixture(url,quantity=3){
  const {m,rpc}=await workspaceClient(url);
  const rack=(await rpc({oneofKind:'write',write:m.Write.create({kind:m.AssetKind.RACK,changes:[{name:'name',value:{oneofKind:'text',text:'Receiving rack'}}]})})).mutation.targets[0];
  const plan=(await rpc({oneofKind:'physical',physical:m.Physical.create({action:{oneofKind:'save',save:{plan:{name:'Unpack three nodes',materials:[{name:'Ordered compute node',quantity,kind:m.AssetKind.DEVICE}]}}}})})).physical.plan;
  return {m,rpc,rack,plan};
}
async function openPlan(page,url,f){
  await page.goto(`${url}/#/?view=2&asset=${f.rack.assetId}&incarnation=${f.rack.incarnation}&scope=${f.rack.assetId}&scopeInc=${f.rack.incarnation}`);
  const planner=page.locator('.physical-planner');
  if(await planner.getAttribute('open')===null)await page.getByText('Plan installation from enclosing scope',{exact:true}).click();
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
    await expect(planner).not.toHaveAttribute('inert','');
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
    await editor.getByRole('button',{name:'Open received device',exact:true}).click();
    await expect(page.getByRole('region',{name:'Resource details',exact:true}).getByRole('heading',{name:'Unpacked node one',exact:true})).toBeVisible();
    await page.getByRole('button',{name:'Close resource details',exact:true}).click();
    await expect(planner).toHaveAttribute('open','');
    await planner.getByText('Required materials and receiving (1 lines)',{exact:true}).click();
    await expect(planner).toContainText('Unit 1: Unpacked node one');
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
    const f=await fixture(url,5);await page.setViewportSize({width:390,height:900});
    const planner=await openPlan(page,url,f);
    await planner.getByRole('button',{name:'Create received device',exact:true}).click();
    const editor=planner.getByRole('region',{name:'Create received device',exact:true});
    await editor.getByLabel('Optional delivery reference or explanation',{exact:true}).fill('Delivery batch A');
    await editor.getByLabel('Use this reference for the next received device',{exact:true}).check();
    for(const name of ['First accepted node','Second accepted node']){
      await expect(planner).not.toHaveAttribute('inert','');
      await editor.getByLabel('Name',{exact:true}).fill(name);
      await editor.getByRole('button',{name:'Record and next',exact:true}).click();
      await expect(editor.getByLabel('Name',{exact:true})).toHaveValue('Ordered compute node');
      await expect(editor.getByLabel('Optional delivery reference or explanation',{exact:true})).toHaveValue('Delivery batch A');
    }
    await expect(planner).not.toHaveAttribute('inert','');
    await editor.getByLabel('Name',{exact:true}).fill('Reply lost node');
    let lossFinished;
    const loss=new Promise(resolve=>{lossFinished=resolve;});
    const lose=async route=>{
      const request=f.m.Request.fromBinary(route.request().postDataBuffer().subarray(5));
      if(request.command.oneofKind==='physical'&&request.command.physical.action.oneofKind==='receiveDevice'){
        expect(request.command.physical.action.receiveDevice.changes.find(v=>v.name==='name').value.text).toBe('Reply lost node');
        await route.fetch();await route.abort('failed');lossFinished();
      }else await route.continue();
    };
    await page.route('**/omloom.workspace.v1.Workspace/Call',lose);
    await editor.getByRole('button',{name:'Record received device',exact:true}).click();
    await expect(editor.getByRole('button',{name:'Recover receipt result',exact:true})).toBeVisible();
    await expect(editor.getByRole('button',{name:'Create in inventory only instead',exact:true})).toHaveCount(0);
    await loss;
    const reference=await page.evaluate(()=>JSON.parse(localStorage.getItem('omloom.pending-device-receipt')));
    expect(Object.keys(reference).sort()).toEqual(['plan','revision']);
    await page.unroute('**/omloom.workspace.v1.Workspace/Call',lose);
    await page.reload();const reopened=await openPlan(page,url,f);
    await reopened.getByRole('button',{name:'Create received device',exact:true}).click();
    await reopened.getByRole('button',{name:'Recover receipt result',exact:true}).click();
    await expect(reopened).toContainText('Receipt accepted');
    const devices=(await f.rpc({oneofKind:'query',query:f.m.Query.create({kinds:[f.m.AssetKind.DEVICE],columns:['name'],limit:25})})).page;
    expect(devices.total).toBe(3n);
    expect(devices.items.map(i=>i.fields.find(v=>v.name==='name').value.text).sort()).toEqual(['First accepted node','Reply lost node','Second accepted node']);
    const loaded=(await f.rpc({oneofKind:'physical',physical:f.m.Physical.create({action:{oneofKind:'load',load:{id:f.plan.id}}})})).physical.plan;
    expect(loaded.materials[0].allocations.map(a=>a.unit)).toEqual([0,1,2]);
    expect(loaded.materials[0].quantity).toBe(5);
  });
});

test('record-next focuses its new form after the enclosing refresh',async({page})=>{
  // Keep the parent read pending until the next editor has prepared. Its initial
  // focus must wait for the enclosing inert state to end, without losing input.
  await withFieldHost(async url=>{
    const f=await fixture(url);const planner=await openPlan(page,url,f);
    await planner.getByRole('button',{name:'Create received device',exact:true}).click();
    const editor=planner.getByRole('region',{name:'Create received device',exact:true});
    await editor.getByLabel('Name',{exact:true}).fill('First device');
    let releaseRead,releasePrepare;
    const readGate=new Promise(resolve=>{releaseRead=resolve;});
    const prepareGate=new Promise(resolve=>{releasePrepare=resolve;});
    const hold=async route=>{
      const command=f.m.Request.fromBinary(route.request().postDataBuffer().subarray(5)).command;
      if(command.oneofKind==='query')await readGate;
      if(command.oneofKind==='prepare')await prepareGate;
      await route.continue();
    };
    await page.route('**/omloom.workspace.v1.Workspace/Call',hold);
    try{
      await editor.getByRole('button',{name:'Record and next',exact:true}).click();
      await expect(planner).toHaveAttribute('inert','');
      releasePrepare();
      await expect(editor.getByLabel('Name',{exact:true})).toHaveValue('Ordered compute node');
      releaseRead();
      await expect(planner).not.toHaveAttribute('inert','');
      await expect(editor.getByLabel('Name',{exact:true})).toBeFocused();
      await page.keyboard.type(' refined');
      await expect(editor.getByLabel('Name',{exact:true})).toHaveValue('Ordered compute node refined');
    }finally{releaseRead();releasePrepare();await page.unroute('**/omloom.workspace.v1.Workspace/Call',hold);}
  });
});

test('a rejected receipt retains input for an explicit inventory-only action',async({page})=>{
  // An intervening commit invalidates the reviewed receipt. Only a definitive
  // rejection permits a separate creation, with the same actual input retained.
  await withFieldHost(async url=>{
    const f=await fixture(url);await page.setViewportSize({width:390,height:900});
    const planner=await openPlan(page,url,f);
    await planner.getByRole('button',{name:'Create received device',exact:true}).click();
    const editor=planner.getByRole('region',{name:'Create received device',exact:true});
    await editor.getByLabel('Name',{exact:true}).fill('Keep actual input');
    await editor.getByLabel('Serial number change',{exact:true}).selectOption('set');
    await editor.getByLabel('Serial number',{exact:true}).fill('ACTUAL-SERIAL');
    await f.rpc({oneofKind:'write',write:f.m.Write.create({kind:f.m.AssetKind.DEVICE,changes:[{name:'name',value:{oneofKind:'text',text:'Concurrent inventory'}}]})});
    await editor.getByRole('button',{name:'Record received device',exact:true}).click();
    await expect(editor).toContainText('Nothing was created by this rejected receipt');
    await expect(editor.getByLabel('Name',{exact:true})).toHaveValue('Keep actual input');
    await editor.getByRole('button',{name:'Create in inventory only instead',exact:true}).click();
    await expect(editor.getByLabel('Serial number',{exact:true})).toHaveValue('ACTUAL-SERIAL');
    await editor.getByRole('button',{name:'Save resource',exact:true}).click();
    await expect(editor).toContainText('Device created in inventory only. The plan was not changed.');
    const loaded=(await f.rpc({oneofKind:'physical',physical:f.m.Physical.create({action:{oneofKind:'load',load:{id:f.plan.id}}})})).physical.plan;
    expect(loaded.materials[0].allocations).toEqual([]);
    const devices=(await f.rpc({oneofKind:'query',query:f.m.Query.create({kinds:[f.m.AssetKind.DEVICE],columns:['name','serial'],limit:25})})).page;
    expect(devices.total).toBe(2n);
    expect(devices.items.find(i=>i.fields.some(v=>v.name==='name'&&v.value.text==='Keep actual input')).fields.find(v=>v.name==='serial').value.text).toBe('ACTUAL-SERIAL');
  });
});

test('late creation preparation does not steal receipt input focus',async({page})=>{
  // The optional reference is usable before ordinary creation choices arrive.
  // Late autofocus must not redirect typing into the suggested device name.
  await withFieldHost(async url=>{
    const f=await fixture(url);const planner=await openPlan(page,url,f);
    let release;const ready=new Promise(resolve=>{release=resolve;});
    const hold=async route=>{
      const request=f.m.Request.fromBinary(route.request().postDataBuffer().subarray(5));
      if(request.command.oneofKind==='prepare')await ready;
      await route.continue();
    };
    await page.route('**/omloom.workspace.v1.Workspace/Call',hold);
    try{
      await planner.getByRole('button',{name:'Create received device',exact:true}).click();
      const editor=planner.getByRole('region',{name:'Create received device',exact:true});
      const reference=editor.getByLabel('Optional delivery reference or explanation',{exact:true});
      await reference.fill('Actual delivery reference');
      release();
      await expect(editor.getByRole('button',{name:'Record received device',exact:true})).toBeEnabled();
      await expect(reference).toBeFocused();
      await expect(reference).toHaveValue('Actual delivery reference');
      await expect(editor.getByLabel('Name',{exact:true})).toHaveValue('Ordered compute node');
      await page.keyboard.type(' / checked');
      await expect(reference).toHaveValue('Actual delivery reference / checked');
      await editor.getByLabel('Name',{exact:true}).focus();
      await page.keyboard.press('ControlOrMeta+A');
      await page.keyboard.type('Keyboard received node');
      await page.keyboard.press('Enter');
      await expect(planner).toContainText('Unit 1: Keyboard received node');
    }finally{release();await page.unroute('**/omloom.workspace.v1.Workspace/Call',hold);}
  });
});

test('inline receiving removes the separate creation and search detour',async({page})=>{
  // Compare actual completed journeys, including their writes and picker search,
  // without claiming a percentage improvement from a synthetic browser run.
  await withFieldHost(async url=>{
    const f=await fixture(url);const requests=[];
    page.on('request',request=>{
      if(request.url().endsWith('/omloom.workspace.v1.Workspace/Call')){
        const command=f.m.Request.fromBinary(request.postDataBuffer().subarray(5)).command;
        requests.push(command.oneofKind==='physical'?`physical.${command.physical.action.oneofKind}`:command.oneofKind);
      }
    });
    await page.goto(`${url}/#/?view=2`);
    await page.getByRole('button',{name:'+ Create resource',exact:true}).click();
    const dialog=page.getByRole('dialog');
    await dialog.getByLabel('Resource kind',{exact:true}).selectOption({label:'device'});
    await dialog.getByLabel('Name',{exact:true}).fill('Separate creation node');
    await dialog.getByRole('button',{name:'Save resource',exact:true}).click();
    await expect(dialog).toBeHidden();
    await expect(page.getByRole('heading',{name:'Separate creation node',exact:true})).toBeVisible();
    const planner=await openPlan(page,url,f);
    const receiving=planner.getByRole('form',{name:'Material receiving',exact:true});
    await receiving.getByLabel('Search permitted inventory outside this selection',{exact:true}).check();
    await receiving.getByRole('combobox',{name:'Find material',exact:true}).fill('Separate creation node');
    await receiving.getByRole('option',{name:'Use material: Separate creation node',exact:true}).click();
    await receiving.getByLabel('Receipt or reuse basis',{exact:true}).fill('Received and selected separately');
    await receiving.getByRole('button',{name:'Record actual material allocation',exact:true}).click();
    await expect(planner).toContainText('Unit 1: Separate creation node');
    await planner.getByRole('button',{name:'Save installation plan',exact:true}).click();
    await expect(planner.getByRole('status')).toHaveText('Plan saved. Nothing is reserved or installed.');
    const baseline=[...requests];requests.length=0;
    await planner.getByRole('button',{name:'Receive or reuse a unit for requirement 1',exact:true}).click();
    await planner.getByRole('button',{name:'Create received device',exact:true}).click();
    const editor=planner.getByRole('region',{name:'Create received device',exact:true});
    await editor.getByLabel('Name',{exact:true}).fill('Inline creation node');
    await editor.getByRole('button',{name:'Record received device',exact:true}).click();
    await expect(planner).toContainText('Unit 2: Inline creation node');
    expect(baseline.filter(v=>v==='write')).toHaveLength(1);
    expect(baseline.filter(v=>v==='physical.save')).toHaveLength(1);
    expect(requests.filter(v=>v==='physical.receiveDevice')).toHaveLength(1);
    expect(requests.filter(v=>v==='write'||v==='physical.receive')).toEqual([]);
    await test.info().attach('completed-receiving-journeys',{body:JSON.stringify({baseline,inline:requests},null,2),contentType:'application/json'});
  });
});
