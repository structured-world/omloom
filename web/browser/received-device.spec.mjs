import {test,expect} from '@playwright/test';
import {withFieldHost} from './field-host.mjs';
import {workspaceClient} from './workspace-client.mjs';

async function fixture(url,quantity=3,specifications=[]){
  const {m,rpc}=await workspaceClient(url);
  const rack=(await rpc({oneofKind:'write',write:m.Write.create({kind:m.AssetKind.RACK,changes:[{name:'name',value:{oneofKind:'text',text:'Receiving rack'}}]})})).mutation.targets[0];
  const plan=(await rpc({oneofKind:'physical',physical:m.Physical.create({action:{oneofKind:'save',save:{plan:{name:'Unpack three nodes',materials:[{name:'Ordered compute node',quantity,kind:m.AssetKind.DEVICE,specifications}]}}}})})).physical.plan;
  return {m,rpc,rack,plan};
}
async function openPlan(page,url,f){
  await page.goto(`${url}/#/?view=2&asset=${f.rack.assetId}&incarnation=${f.rack.incarnation}&scope=${f.rack.assetId}&scopeInc=${f.rack.incarnation}`);
  const planner=page.locator('.physical-planner');
  if(await planner.getAttribute('open')===null)await page.getByText('Plan installation from enclosing scope',{exact:true}).click();
  await planner.getByRole('button',{name:'Unpack three nodes · draft',exact:true}).click();
  await planner.getByText(`Required materials and receiving (${f.plan.materials.length} lines)`,{exact:true}).click();
  await planner.getByRole('button',{name:'Receive or reuse a unit for requirement 1',exact:true}).click();
  return planner;
}

test('record-and-next validates required input before dispatch',async({page})=>{
  // The secondary submit action must use the same browser constraints as the
  // main action, without claiming recovery storage or sending invalid input.
  await withFieldHost(async url=>{
    const f=await fixture(url),planner=await openPlan(page,url,f);let submissions=0;
    page.on('request',request=>{
      const bytes=request.postDataBuffer();if(!bytes||!request.url().endsWith('/omloom.workspace.v1.Workspace/Call'))return;
      const command=f.m.Request.fromBinary(bytes.subarray(5)).command;
      if(command.oneofKind==='physical'&&command.physical.action.oneofKind==='receiveDevice')submissions++;
    });
    await planner.getByRole('button',{name:'Create received device',exact:true}).click();
    const editor=planner.getByRole('region',{name:'Create received device',exact:true}),name=editor.getByLabel('Name',{exact:true});
    await name.fill('');
    await editor.getByRole('button',{name:'Record and next',exact:true}).click();
    await expect(name).toBeFocused();expect(submissions).toBe(0);
    await name.fill('Valid received device');
    await editor.getByRole('button',{name:'Record and next',exact:true}).click();
    await expect(planner).toContainText('Unit 1: Valid received device');
    await expect(editor).toContainText('Unit 2');expect(submissions).toBe(1);
  });
});

for(const line of [0,1])test(`unapplied requirement ${line+1} input survives receipt completion`,async({page})=>{
  // A receipt updates its own allocation, not an unrelated open requirement's
  // unsaved input. Applying that input afterward must preserve the receipt too.
  await withFieldHost(async url=>{
    const f=await fixture(url),plan=f.m.PhysicalPlan.clone(f.plan);
    plan.materials.push(f.m.PhysicalMaterial.create({name:'Additional demand',kind:f.m.AssetKind.DEVICE,quantity:1}));
    f.plan=(await f.rpc({oneofKind:'physical',physical:f.m.Physical.create({action:{oneofKind:'save',save:{plan}}})})).physical.plan;
    const planner=await openPlan(page,url,f);
    await planner.getByRole('button',{name:'Create received device',exact:true}).click();
    const editor=planner.getByRole('region',{name:'Create received device',exact:true});
    let release,started;const gate=new Promise(resolve=>{release=resolve;}),sent=new Promise(resolve=>{started=resolve;});
    const hold=async route=>{
      const command=f.m.Request.fromBinary(route.request().postDataBuffer().subarray(5)).command;
      if(command.oneofKind==='physical'&&command.physical.action.oneofKind==='receiveDevice'){
        started();await gate;
      }
      await route.continue();
    };
    await page.route('**/omloom.workspace.v1.Workspace/Call',hold);
    try{
      await editor.getByRole('button',{name:'Record received device',exact:true}).click();await sent;
      await planner.getByRole('button',{name:`Edit requirement ${line+1}`,exact:true}).click();
      const draft=planner.getByRole('form',{name:'Material requirement editor',exact:true});
      await draft.getByLabel('Material requirement name',{exact:true}).fill('Unapplied new demand');
      await draft.getByLabel('Required quantity',{exact:true}).fill('7');release();
      await expect(editor).toContainText('Receipt accepted');
      await expect(draft.getByLabel('Material requirement name',{exact:true})).toHaveValue('Unapplied new demand');
      await expect(draft.getByLabel('Required quantity',{exact:true})).toHaveValue('7');
      await draft.getByRole('button',{name:'Apply material requirement',exact:true}).click();
      await planner.getByRole('button',{name:'Save installation plan',exact:true}).click();
      await expect(planner).toContainText('Plan saved. Nothing is reserved or installed.');
      const loaded=(await f.rpc({oneofKind:'physical',physical:f.m.Physical.create({action:{oneofKind:'load',load:{id:f.plan.id}}})})).physical.plan;
      expect(loaded.materials[line].name).toBe('Unapplied new demand');
      expect(loaded.materials[line].quantity).toBe(7);
      expect(loaded.materials[0].allocations.map(a=>a.unit)).toEqual([0]);
    }finally{release();await page.unroute('**/omloom.workspace.v1.Workspace/Call',hold);}
  });
});

test('allocating existing equipment clears revision-bound reviews',async({page})=>{
  // Allocation changes inventory even though it bypasses the ordinary planner
  // command helper. Old hypotheses and quality results must disappear together.
  await withFieldHost(async url=>{
    const f=await fixture(url),{m,rpc}=f;
    const create=async name=>(await rpc({oneofKind:'write',write:m.Write.create({kind:m.AssetKind.DEVICE,changes:[{name:'name',value:{oneofKind:'text',text:name}}]})})).mutation.targets[0];
    const old=await create('Installed predecessor'),incoming=await create('Actual received node');
    await rpc({oneofKind:'relation',relation:m.Relationship.create({relation:{from:old,to:f.rack,kind:m.RelationKind.INSTALLED_IN}})});
    const binding=(await rpc({oneofKind:'read',read:m.Read.create({target:old})})).detail.relations.find(r=>r.kind===m.RelationKind.INSTALLED_IN);
    const draft=m.PhysicalPlan.clone(f.plan);
    draft.steps=[m.PhysicalStep.create({action:{oneofKind:'replacement',replacement:{position:f.rack,previous:old,previousBinding:binding.relationId,previousRevision:binding.revision}}})];
    f.plan=(await rpc({oneofKind:'physical',physical:m.Physical.create({action:{oneofKind:'save',save:{plan:draft}}})})).physical.plan;
    const planner=await openPlan(page,url,f);
    await planner.getByLabel('Reserve until',{exact:true}).fill(await page.evaluate(()=>{const date=new Date(Date.now()+3600000);date.setMinutes(date.getMinutes()-date.getTimezoneOffset());return date.toISOString().slice(0,16);}));
    await planner.getByRole('button',{name:'Check and reserve capacity',exact:true}).click();
    await expect(planner).toContainText('Capacity reserved. Installation is not confirmed.');
    const review=planner.locator('.replacement-review');
    await review.getByText('Replacement evidence and event decision',{exact:true}).click();
    await review.getByRole('button',{name:'Explain current replacement hypothesis',exact:true}).click();
    await expect(review.locator('h4')).toBeVisible();
    await planner.getByText('Local replacement review quality',{exact:true}).click();
    await planner.getByRole('button',{name:'Load current review counts',exact:true}).click();
    await expect(planner.getByText('No retained judgments in this contributing scope.',{exact:true})).toBeVisible();
    await planner.getByRole('button',{name:'Receive or reuse a unit for requirement 1',exact:true}).click();
    const receiving=planner.getByRole('form',{name:'Material receiving',exact:true});
    await receiving.getByLabel('Search permitted inventory outside this selection',{exact:true}).check();
    await receiving.getByRole('combobox',{name:'Find material',exact:true}).fill('Actual received node');
    await receiving.getByRole('option',{name:'Use material: Actual received node',exact:true}).click();
    await receiving.getByLabel('Receipt or reuse basis',{exact:true}).fill('Received actual equipment');
    await receiving.getByRole('button',{name:'Record actual material allocation',exact:true}).click();
    await expect(planner).toContainText('Actual material allocation recorded. Nothing was installed.');
    await expect(review.locator('h4')).toHaveCount(0);
    await expect(planner.getByText('No retained judgments in this contributing scope.',{exact:true})).toHaveCount(0);
    expect((await rpc({oneofKind:'physical',physical:m.Physical.create({action:{oneofKind:'load',load:{id:f.plan.id}}})})).physical.plan.materials[0].allocations[0].item).toEqual(incoming);
  });
});

test('an unavailable URL plan leaves other saved plans usable',async({page})=>{
  // A failed optional restoration must not lock the whole planner or repeat
  // the same failed load whenever the details control is opened.
  await withFieldHost(async url=>{
    const f=await fixture(url),missing=crypto.randomUUID();let attempts=0;
    page.on('request',request=>{
      const bytes=request.postDataBuffer();if(!bytes||!request.url().endsWith('/omloom.workspace.v1.Workspace/Call'))return;
      const command=f.m.Request.fromBinary(bytes.subarray(5)).command;
      if(command.oneofKind==='physical'&&command.physical.action.oneofKind==='load'&&command.physical.action.load.id===missing)attempts++;
    });
    await page.goto(`${url}/#/?view=2&asset=${f.rack.assetId}&incarnation=${f.rack.incarnation}&scope=${f.rack.assetId}&scopeInc=${f.rack.incarnation}&physicalPlan=${missing}`);
    const planner=page.locator('.physical-planner');
    await expect(page.getByText('Plan installation from enclosing scope',{exact:true})).toBeVisible();
    await expect(planner.getByRole('alert')).toBeVisible();
    await expect(planner.getByRole('button',{name:'New installation plan',exact:true})).toBeEnabled();
    await planner.getByRole('button',{name:'Unpack three nodes · draft',exact:true}).click();
    await expect(planner.getByLabel('Installation plan name',{exact:true})).toHaveValue('Unpack three nodes');
    await page.getByText('Plan installation from enclosing scope',{exact:true}).click();
    await page.getByText('Plan installation from enclosing scope',{exact:true}).click();
    expect(attempts).toBe(1);
    expect(page.url()).not.toContain(`physicalPlan=${missing}`);
  });
});

for(const width of [1440,390])test(`received devices remain unplaced and record-next clears serial at ${width}px`,async({page})=>{
  // The existing inventory editor creates actual fields and allocation in one commit,
  // without a create/search detour, copied serial, or fabricated rack attachment.
  await withFieldHost(async url=>{
    const f=await fixture(url,3,[{name:'width',value:{oneofKind:'equipment',equipment:{basis:'Planned supplier width',assertion:{oneofKind:'quantity',quantity:{upper:500000n,unit:'mm',scale:3}}}}}]);await page.setViewportSize({width,height:900});
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
      if(request.command.oneofKind==='physical'&&request.command.physical.action.oneofKind==='recoverReceipt'){await route.abort('failed');return;}
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
    const reference=await page.evaluate(()=>new Promise((resolve,reject)=>{
      const request=indexedDB.open('omloom-receipt-recovery');
      request.onerror=()=>reject(request.error);
      request.onsuccess=()=>{const db=request.result,tx=db.transaction('pending');const read=tx.objectStore('pending').openCursor();read.onsuccess=()=>resolve(read.result.value);tx.oncomplete=()=>db.close();};
    }));
    expect(Object.keys(reference).sort()).toEqual(['material','plan','revision','unit']);
    await page.unroute('**/omloom.workspace.v1.Workspace/Call',lose);
    await page.reload();const reopened=await openPlan(page,url,f);
    await reopened.getByRole('button',{name:'Create received device',exact:true}).click();
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

test('removing the selected material closes receiving without a render error',async({page})=>{
  // A draft removal must not leave the editor rendering an absent material.
  await withFieldHost(async url=>{
    const f=await fixture(url);const errors=[];page.on('pageerror',error=>errors.push(error.message));
    page.on('console',message=>{if(message.type()==='error')errors.push(message.text());});
    const planner=await openPlan(page,url,f);
    await planner.getByRole('button',{name:'Create received device',exact:true}).click();
    await planner.getByRole('region',{name:'Create received device',exact:true}).getByLabel('Name',{exact:true}).fill('Unsubmitted item');
    await planner.getByRole('button',{name:'Remove requirement 1',exact:true}).click();
    await expect(planner.getByRole('region',{name:'Create received device',exact:true})).toHaveCount(0);
    await expect(planner).toContainText('Required materials and receiving (0 lines)');
    await planner.getByRole('button',{name:'Add material requirement',exact:true}).click();
    await expect(planner.getByRole('form',{name:'Material requirement editor',exact:true})).toBeVisible();
    await page.evaluate(()=>new Promise(requestAnimationFrame));
    expect(errors).toEqual([]);
  });
});

test('a C1 control in a delivery reference never dispatches a receipt',async({page})=>{
  // Client validation must cover the same control range as the server, before
  // persisting an unresolved reference or sending a mutation.
  await withFieldHost(async url=>{
    const f=await fixture(url);const planner=await openPlan(page,url,f);const sent=[];
    page.on('request',request=>{if(request.url().endsWith('/omloom.workspace.v1.Workspace/Call')){
      const command=f.m.Request.fromBinary(request.postDataBuffer().subarray(5)).command;
      if(command.oneofKind==='physical'&&command.physical.action.oneofKind==='receiveDevice')sent.push(command);
    }});
    await planner.getByRole('button',{name:'Create received device',exact:true}).click();
    const editor=planner.getByRole('region',{name:'Create received device',exact:true});
    await editor.getByLabel('Name',{exact:true}).fill('Received device');
    await editor.getByLabel('Optional delivery reference or explanation',{exact:true}).fill('delivery\u009c');
    await editor.getByRole('button',{name:'Record received device',exact:true}).click();
    await expect(editor.getByRole('alert')).toContainText('without control characters');
    expect(sent).toEqual([]);
    await expect(editor.getByRole('button',{name:'Recover receipt result',exact:true})).toHaveCount(0);
  });
});

test('explicit review prepares a rejected receipt without losing actual input',async({page})=>{
  // A fresh application revision needs explicit preparation, not an automatic retry.
  await withFieldHost(async url=>{
    const f=await fixture(url);const planner=await openPlan(page,url,f);
    await planner.getByRole('button',{name:'Create received device',exact:true}).click();
    const editor=planner.getByRole('region',{name:'Create received device',exact:true});
    await editor.getByLabel('Name',{exact:true}).fill('Retained actual name');
    await editor.getByLabel('Serial number change',{exact:true}).selectOption('set');
    await editor.getByLabel('Serial number',{exact:true}).fill('REVIEWED-SERIAL');
    await f.rpc({oneofKind:'write',write:f.m.Write.create({kind:f.m.AssetKind.DEVICE,changes:[{name:'name',value:{oneofKind:'text',text:'Concurrent edit'}}]})});
    await editor.getByRole('button',{name:'Record received device',exact:true}).click();
    await expect(editor).toContainText('Nothing was created by this rejected receipt');
    await editor.getByRole('button',{name:'Review current receipt',exact:true}).click();
    await expect(editor.getByLabel('Name',{exact:true})).toHaveValue('Retained actual name');
    await expect(editor.getByLabel('Serial number',{exact:true})).toHaveValue('REVIEWED-SERIAL');
    await editor.getByRole('button',{name:'Record received device',exact:true}).click();
    await expect(planner).toContainText('Unit 1: Retained actual name');
  });
});

test('an accepted receipt is applied after an overlapping planner read',async({page})=>{
  // The server has committed, but an unrelated list read is still pending when
  // the receipt response reaches the UI. It must still advance record-next.
  await withFieldHost(async url=>{
    const f=await fixture(url);const planner=await openPlan(page,url,f);
    await planner.getByRole('button',{name:'Create received device',exact:true}).click();
    const editor=planner.getByRole('region',{name:'Create received device',exact:true});
    await editor.getByLabel('Name',{exact:true}).fill('Accepted while reading');
    let releaseReceipt,releaseRead,accepted,reading;
    const receiptGate=new Promise(resolve=>{releaseReceipt=resolve;}),readGate=new Promise(resolve=>{releaseRead=resolve;});
    const committed=new Promise(resolve=>{accepted=resolve;}),readStarted=new Promise(resolve=>{reading=resolve;});
    const hold=async route=>{
      const command=f.m.Request.fromBinary(route.request().postDataBuffer().subarray(5)).command;
      if(command.oneofKind==='physical'&&command.physical.action.oneofKind==='receiveDevice'){
        const response=await route.fetch();accepted();await receiptGate;await route.fulfill({response});return;
      }
      if(command.oneofKind==='physical'&&command.physical.action.oneofKind==='list'){reading();await readGate;}
      await route.continue();
    };
    await page.route('**/omloom.workspace.v1.Workspace/Call',hold);
    try{
      await editor.getByRole('button',{name:'Record and next',exact:true}).click();await committed;
      await planner.getByRole('button',{name:'First saved plans',exact:true}).click();await readStarted;
      releaseReceipt();await expect(editor).toContainText('Receipt accepted');
      releaseRead();
      await expect(planner).toContainText('Unit 1: Accepted while reading');
      await expect(editor).toContainText('Unit 2');
      await expect(planner).not.toHaveAttribute('inert','');
      await expect(editor.getByLabel('Name',{exact:true})).toHaveValue('Ordered compute node');
    }finally{releaseReceipt();releaseRead();await page.unroute('**/omloom.workspace.v1.Workspace/Call',hold);}
  });
});

test('receipt acceptance preserves in-flight plan edits without saving them',async({page})=>{
  // Receipt commits only the reviewed allocation, never unrelated draft input.
  await withFieldHost(async url=>{
    const f=await fixture(url);const planner=await openPlan(page,url,f);
    await planner.getByRole('button',{name:'Create received device',exact:true}).click();
    const editor=planner.getByRole('region',{name:'Create received device',exact:true});
    await editor.getByLabel('Name',{exact:true}).fill('Received alongside draft');
    let release,accepted;
    const gate=new Promise(resolve=>{release=resolve;}),committed=new Promise(resolve=>{accepted=resolve;});
    const hold=async route=>{
      const command=f.m.Request.fromBinary(route.request().postDataBuffer().subarray(5)).command;
      if(command.oneofKind==='physical'&&command.physical.action.oneofKind==='receiveDevice'){
        const response=await route.fetch();accepted();await gate;await route.fulfill({response});
      }else await route.continue();
    };
    await page.route('**/omloom.workspace.v1.Workspace/Call',hold);
    try{
      await editor.getByRole('button',{name:'Record received device',exact:true}).click();await committed;
      await planner.getByLabel('Installation plan name',{exact:true}).fill('Keep my draft name');
      release();await expect(planner).toContainText('Unit 1: Received alongside draft');
      await expect(planner.getByLabel('Installation plan name',{exact:true})).toHaveValue('Keep my draft name');
      const saved=()=>f.rpc({oneofKind:'physical',physical:f.m.Physical.create({action:{oneofKind:'load',load:{id:f.plan.id}}})});
      expect((await saved()).physical.plan.name).toBe('Unpack three nodes');
      await planner.getByRole('button',{name:'Save installation plan',exact:true}).click();
      await expect(planner.getByRole('status').filter({hasText:'Plan saved.'})).toHaveText('Plan saved. Nothing is reserved or installed.');
      const result=(await saved()).physical.plan;
      expect(result.name).toBe('Keep my draft name');expect(result.materials[0].allocations).toHaveLength(1);
    }finally{release();await page.unroute('**/omloom.workspace.v1.Workspace/Call',hold);}
  });
});

test('a dirty plan receipt advances the receiving selection',async({page})=>{
  // Rebasing an accepted allocation into an edited plan must not leave the
  // receiving form pointing at the unit that was just consumed.
  await withFieldHost(async url=>{
    const f=await fixture(url),planner=await openPlan(page,url,f);
    await planner.getByRole('button',{name:'Create received device',exact:true}).click();
    const editor=planner.getByRole('region',{name:'Create received device',exact:true});
    let release,started;const gate=new Promise(resolve=>{release=resolve;}),sent=new Promise(resolve=>{started=resolve;});
    const hold=async route=>{
      const command=f.m.Request.fromBinary(route.request().postDataBuffer().subarray(5)).command;
      if(command.oneofKind==='physical'&&command.physical.action.oneofKind==='receiveDevice'){started();await gate;}
      await route.continue();
    };
    await page.route('**/omloom.workspace.v1.Workspace/Call',hold);
    try{
      await editor.getByRole('button',{name:'Record received device',exact:true}).click();await sent;
      await planner.getByLabel('Installation plan name',{exact:true}).fill('Edited receiving plan');release();
      await expect(editor).toContainText('Receipt accepted');
      await planner.getByRole('button',{name:'Save installation plan',exact:true}).click();
      await expect(planner).toContainText('Plan saved. Nothing is reserved or installed.');
      await editor.getByRole('button',{name:'Close receiving',exact:true}).click();
      await expect(planner.getByLabel('Requested unit number',{exact:true})).toHaveValue('2');
      await planner.getByRole('button',{name:'Create received device',exact:true}).click();
      await editor.getByRole('button',{name:'Record received device',exact:true}).click();
      await expect(planner).toContainText('2 allocated of 3 requested');
      const saved=(await f.rpc({oneofKind:'physical',physical:f.m.Physical.create({action:{oneofKind:'load',load:{id:f.plan.id}}})})).physical.plan;
      expect(saved.materials[0].allocations.map(a=>a.unit)).toEqual([0,1]);
    }finally{release();await page.unroute('**/omloom.workspace.v1.Workspace/Call',hold);}
  });
});

test('receipt acceptance never retargets a changed draft requirement',async({page})=>{
  // A changed line has no inferred identity. Keep the draft, surface the accepted
  // result, and prevent an unaware save from removing its confirmed allocation.
  await withFieldHost(async url=>{
    const f=await fixture(url);const planner=await openPlan(page,url,f);
    await planner.getByRole('button',{name:'Create received device',exact:true}).click();
    const editor=planner.getByRole('region',{name:'Create received device',exact:true});
    await editor.getByLabel('Name',{exact:true}).fill('Confirmed old requirement');
    await planner.getByRole('button',{name:'Edit requirement 1',exact:true}).click();
    await planner.getByLabel('Material requirement name',{exact:true}).fill('Changed draft requirement');
    await planner.getByRole('button',{name:'Apply material requirement',exact:true}).click();
    await editor.getByRole('button',{name:'Record received device',exact:true}).click();
    await expect(editor).toContainText('Receipt accepted');
    await expect(planner.getByRole('alert')).toContainText('cannot be reconciled automatically');
    await expect(planner).toContainText('Changed draft requirement');
    await planner.getByRole('button',{name:'Save installation plan',exact:true}).click();
    await expect(planner.getByRole('alert').filter({hasText:'Reopen the saved plan before'})).toBeVisible();
    const loaded=(await f.rpc({oneofKind:'physical',physical:f.m.Physical.create({action:{oneofKind:'load',load:{id:f.plan.id}}})})).physical.plan;
    expect(loaded.materials[0].name).toBe('Ordered compute node');expect(loaded.materials[0].allocations).toHaveLength(1);
  });
});

test('cancelled plans do not offer receipt from a waiting placement',async({page})=>{
  // A retained unallocated step is not a receivable plan after cancellation.
  await withFieldHost(async url=>{
    const f=await fixture(url);
    const extent={startUm:0n,lengthUm:1000n};
    f.plan=(await f.rpc({oneofKind:'physical',physical:f.m.Physical.create({action:{oneofKind:'save',save:{plan:{...f.plan,steps:[{action:{oneofKind:'placement',placement:{enclosure:f.rack,volume:{x:extent,y:extent,z:extent},material:{material:0,unit:0}}}}]}}}})})).physical.plan;
    const planner=await openPlan(page,url,f);
    await expect(planner.getByRole('button',{name:'Create received device for step 1',exact:true})).toBeVisible();
    await planner.getByRole('button',{name:'Stop remaining work',exact:true}).click();
    await expect(planner).toContainText('Remaining work stopped.');
    await expect(planner.getByRole('button',{name:'Create received device for step 1',exact:true})).toHaveCount(0);
  });
});

test('explicit receipt review refreshes a changed saved plan and retains input',async({page})=>{
  // Inventory preparation alone cannot refresh the separately reviewed plan key.
  await withFieldHost(async url=>{
    const f=await fixture(url);const planner=await openPlan(page,url,f);
    await planner.getByRole('button',{name:'Create received device',exact:true}).click();
    const editor=planner.getByRole('region',{name:'Create received device',exact:true});
    await editor.getByLabel('Name',{exact:true}).fill('Retain through plan review');
    await editor.getByLabel('Serial number change',{exact:true}).selectOption('set');
    await editor.getByLabel('Serial number',{exact:true}).fill('PLAN-REVIEW-SERIAL');
    await f.rpc({oneofKind:'physical',physical:f.m.Physical.create({action:{oneofKind:'save',save:{plan:{...f.plan,name:'Reviewed receiving plan'}}}})});
    await editor.getByRole('button',{name:'Record received device',exact:true}).click();
    await expect(editor).toContainText('Nothing was created by this rejected receipt');
    await editor.getByRole('button',{name:'Review current receipt',exact:true}).click();
    await expect(editor.getByLabel('Serial number',{exact:true})).toHaveValue('PLAN-REVIEW-SERIAL');
    await expect(editor).toContainText('Reviewed receiving plan');
    await editor.getByRole('button',{name:'Record received device',exact:true}).click();
    await expect(planner).toContainText('Unit 1: Retain through plan review');
  });
});

test('another tab cannot erase an unresolved accepted receipt',async({page,context})=>{
  // A second tab cannot submit for the same unit while its result is unresolved.
  // It can recover the original result through transactional browser storage.
  await withFieldHost(async url=>{
    const f=await fixture(url),other=await context.newPage();
    const a=await openPlan(page,url,f),b=await openPlan(other,url,f);
    for(const planner of [a,b])await planner.getByRole('button',{name:'Create received device',exact:true}).click();
    const accepted=a.getByRole('region',{name:'Create received device',exact:true}),loser=b.getByRole('region',{name:'Create received device',exact:true});
    await accepted.getByLabel('Name',{exact:true}).fill('Cross-tab accepted device');
    await loser.getByLabel('Name',{exact:true}).fill('Must not create');
    let finish;const lost=new Promise(resolve=>{finish=resolve;});let submissions=0,allowRecovery=false;
    const intercept=async route=>{
      const command=f.m.Request.fromBinary(route.request().postDataBuffer().subarray(5)).command;
      if(command.oneofKind==='physical'&&['recoverReceipt','receiptOutcome'].includes(command.physical.action.oneofKind)&&!allowRecovery){await route.abort('failed');return;}
      if(command.oneofKind==='physical'&&command.physical.action.oneofKind==='receiveDevice'){
        submissions++;const response=await route.fetch();
        if(route.request().frame().page()===page){await route.abort('failed');finish();}
        else await route.fulfill({response});
      }else await route.continue();
    };
    await context.route('**/omloom.workspace.v1.Workspace/Call',intercept);
    try{
      await accepted.getByRole('button',{name:'Record received device',exact:true}).click();await lost;
      await loser.getByRole('button',{name:'Record received device',exact:true}).click();
      await expect(loser.getByRole('alert').filter({hasText:/Error/})).toBeVisible();
      expect(submissions).toBe(1);
      allowRecovery=true;
      await loser.getByRole('button',{name:'Recover receipt result',exact:true}).click();
      await expect(b).toContainText('Unit 1: Cross-tab accepted device');
      const devices=(await f.rpc({oneofKind:'query',query:f.m.Query.create({kinds:[f.m.AssetKind.DEVICE],columns:['name'],limit:25})})).page;
      expect(devices.total).toBe(1n);
    }finally{await context.unroute('**/omloom.workspace.v1.Workspace/Call',intercept);await other.close();}
  });
});

for(const known of [false,true])test(`a reference resolved in another tab releases this editor (known outcome: ${known})`,async({page,context})=>{
  // Clearing a resolved reference elsewhere must not leave a stale editor hidden.
  // A locally known result still needs its failed parent refresh applied.
  await withFieldHost(async url=>{
    const f=await fixture(url),other=await context.newPage();
    const a=await openPlan(page,url,f);
    await a.getByRole('button',{name:'Create received device',exact:true}).click();
    const editor=a.getByRole('region',{name:'Create received device',exact:true});
    await editor.getByLabel('Name',{exact:true}).fill('Resolved in other tab');
    let submitted=false,failRefresh=true;
    const intercept=async route=>{
      const command=f.m.Request.fromBinary(route.request().postDataBuffer().subarray(5)).command;
      if(failRefresh&&command.oneofKind==='physical'&&command.physical.action.oneofKind==='recoverReceipt'){await route.abort('failed');return;}
      if(command.oneofKind==='physical'&&command.physical.action.oneofKind==='receiveDevice'){
        submitted=true;
        if(!known){await route.fetch();await route.abort('failed');return;}
      }
      if(known&&submitted&&failRefresh&&command.oneofKind==='physical'&&command.physical.action.oneofKind==='load'){await route.abort('failed');return;}
      await route.continue();
    };
    await page.route('**/omloom.workspace.v1.Workspace/Call',intercept);
    try{
      await editor.getByRole('button',{name:'Record received device',exact:true}).click();
      await expect(editor.getByRole('button',{name:'Recover receipt result',exact:true})).toBeVisible();
      const b=await openPlan(other,url,f);
      await b.getByRole('button',{name:'Create received device',exact:true}).click();
      await expect(b).toContainText('Receipt accepted');failRefresh=false;
      await editor.getByRole('button',{name:'Recover receipt result',exact:true}).click();
      await expect(editor.getByRole('button',{name:'Recover receipt result',exact:true})).toHaveCount(0);
      if(known)await expect(a).toContainText('Unit 1: Resolved in other tab');
      else await expect(editor.getByLabel('Name',{exact:true})).toHaveValue('Resolved in other tab');
    }finally{await page.unroute('**/omloom.workspace.v1.Workspace/Call',intercept);await other.close();}
  });
});

for(const width of [1440,390])test(`an unavailable earlier unit does not block another device and can reuse inventory at ${width}px`,async({page})=>{
  // An old unknown save belongs to its exact unit. Another unit remains usable,
  // and resolving the old one selects a real device instead of creating a duplicate.
  await withFieldHost(async url=>{
    await page.setViewportSize({width,height:900});
    const f=await fixture(url);
    const existing=(await f.rpc({oneofKind:'write',write:f.m.Write.create({kind:f.m.AssetKind.DEVICE,changes:[{name:'name',value:{oneofKind:'text',text:'Found original device'}}]})})).mutation.targets[0];
    await f.rpc({oneofKind:'physical',physical:f.m.Physical.create({action:{oneofKind:'save',save:{plan:{name:'Unrelated change',materials:[{name:'Other demand',quantity:1,kind:f.m.AssetKind.DEVICE}]}}}})});
    const planner=await openPlan(page,url,f);
    await page.evaluate(({plan,revision})=>new Promise((resolve,reject)=>{
      const request=indexedDB.open('omloom-receipt-recovery',1);
      request.onupgradeneeded=()=>request.result.createObjectStore('pending',{autoIncrement:true});
      request.onerror=()=>reject(request.error);
      request.onsuccess=()=>{const db=request.result,tx=db.transaction('pending','readwrite');tx.objectStore('pending').add({plan,revision,material:0,unit:0});tx.oncomplete=()=>{db.close();resolve();};tx.onabort=()=>reject(tx.error);};
    }),{plan:f.plan.id,revision:String(f.plan.revision)});
    await planner.getByLabel('Requested unit number',{exact:true}).fill('2');
    await planner.getByRole('button',{name:'Create received device',exact:true}).click();
    const editor=planner.getByRole('region',{name:'Create received device',exact:true});
    await expect(editor).not.toContainText('Check the existing device');
    await editor.getByLabel('Name',{exact:true}).fill('Another received device');
    await editor.getByRole('button',{name:'Record received device',exact:true}).click();
    await expect(planner).toContainText('Unit 2: Another received device');
    await editor.getByRole('button',{name:'Close receiving',exact:true}).click();
    await planner.getByRole('button',{name:'Receive or reuse a unit for requirement 1',exact:true}).click();
    await planner.getByLabel('Requested unit number',{exact:true}).fill('1');
    await planner.getByRole('button',{name:'Create received device',exact:true}).click();
    await expect(editor).toContainText('Check the existing device');
    await editor.getByRole('button',{name:'Find existing device',exact:true}).click();
    await editor.getByRole('combobox',{name:'Find material',exact:true}).fill('Found original device');
    await editor.getByRole('option',{name:'Use material: Found original device',exact:true}).click();
    await editor.getByRole('button',{name:'Use existing device',exact:true}).click();
    await expect(planner).toContainText('Unit 1: Found original device');
    const loaded=(await f.rpc({oneofKind:'physical',physical:f.m.Physical.create({action:{oneofKind:'load',load:{id:f.plan.id}}})})).physical.plan;
    expect(loaded.materials[0].allocations.find(a=>a.unit===0).item).toEqual(existing);
    const devices=(await f.rpc({oneofKind:'query',query:f.m.Query.create({kinds:[f.m.AssetKind.DEVICE],columns:['name'],limit:25})})).page;
    expect(devices.total).toBe(2n);
  });
});

test('opening another unit does not fence an in-flight device save',async({page,context})=>{
  // Admission is per material unit. Merely opening another unit must not close
  // the first tab's unused revision or turn its pending save into a refusal.
  await withFieldHost(async url=>{
    const f=await fixture(url),first=await openPlan(page,url,f),other=await context.newPage();
    await first.getByRole('button',{name:'Create received device',exact:true}).click();
    const editor=first.getByRole('region',{name:'Create received device',exact:true});
    await editor.getByLabel('Name',{exact:true}).fill('First in-flight device');
    let release,started;const gate=new Promise(resolve=>{release=resolve;}),sent=new Promise(resolve=>{started=resolve;});
    let recoveryRequests=0;
    const hold=async route=>{
      const command=f.m.Request.fromBinary(route.request().postDataBuffer().subarray(5)).command;
      if(command.oneofKind==='physical'&&command.physical.action.oneofKind==='recoverReceipt')recoveryRequests++;
      if(command.oneofKind==='physical'&&command.physical.action.oneofKind==='receiveDevice'){
        started();await gate;
      }
      await route.continue();
    };
    await context.route('**/omloom.workspace.v1.Workspace/Call',hold);
    try{
      await editor.getByRole('button',{name:'Record received device',exact:true}).click();await sent;
      const second=await openPlan(other,url,f);
      await second.getByLabel('Requested unit number',{exact:true}).fill('2');
      await second.getByRole('button',{name:'Create received device',exact:true}).click();
      await expect(second.getByRole('button',{name:'Record received device',exact:true})).toBeEnabled();
      expect(recoveryRequests).toBe(0);release();
      await expect(first).toContainText('Unit 1: First in-flight device');
      const loaded=(await f.rpc({oneofKind:'physical',physical:f.m.Physical.create({action:{oneofKind:'load',load:{id:f.plan.id}}})})).physical.plan;
      expect(loaded.materials[0].allocations.map(a=>a.unit)).toEqual([0]);
    }finally{release();await context.unrouteAll({behavior:'wait'});await other.close();}
  });
});

test('simultaneous receipt tabs admit only one unresolved command',async({page,context})=>{
  // The winner retains admission through response delivery, so a second tab
  // cannot dispatch even when both editors were prepared at the same revision.
  await withFieldHost(async url=>{
    const f=await fixture(url),other=await context.newPage();
    const planners=[await openPlan(page,url,f),await openPlan(other,url,f)];
    for(const planner of planners){
      await planner.getByRole('button',{name:'Create received device',exact:true}).click();
      await expect(planner.getByRole('button',{name:'Record received device',exact:true})).toBeEnabled();
    }
    let release,accepted;const gate=new Promise(resolve=>{release=resolve;}),committed=new Promise(resolve=>{accepted=resolve;});let submissions=0;
    const hold=async route=>{
      const command=f.m.Request.fromBinary(route.request().postDataBuffer().subarray(5)).command;
      if(command.oneofKind==='physical'&&command.physical.action.oneofKind==='receiptOutcome')await committed;
      if(command.oneofKind==='physical'&&command.physical.action.oneofKind==='receiveDevice'){
        submissions++;const response=await route.fetch();accepted();await gate;await route.fulfill({response});
      }else await route.continue();
    };
    await context.route('**/omloom.workspace.v1.Workspace/Call',hold);
    try{
      await Promise.all(planners.map(p=>p.getByRole('button',{name:'Record received device',exact:true}).click()));
      await committed;
      await expect.poll(async()=>Promise.all(planners.map(p=>p.getByRole('status').filter({hasText:/^Receipt accepted at revision/}).count())).then(counts=>counts.reduce((a,b)=>a+b,0))).toBe(1);
      expect(submissions).toBe(1);release();
      const devices=(await f.rpc({oneofKind:'query',query:f.m.Query.create({kinds:[f.m.AssetKind.DEVICE],columns:['name'],limit:25})})).page;
      expect(devices.total).toBe(1n);
    }finally{release();await context.unrouteAll({behavior:'wait'});await other.close();}
  });
});

for(const accepted of [false,true])test(`a lost save is checked automatically (committed: ${accepted})`,async({page})=>{
  // The user never coordinates protocol recovery. Only a proven refusal makes
  // the retained input available for an explicitly reviewed fresh submission.
  await withFieldHost(async url=>{
    await page.setViewportSize({width:390,height:900});
    const f=await fixture(url),planner=await openPlan(page,url,f);
    await planner.getByRole('button',{name:'Create received device',exact:true}).click();
    const editor=planner.getByRole('region',{name:'Create received device',exact:true});
    await editor.getByLabel('Name',{exact:true}).fill('Automatically checked device');
    let interrupted=false;
    const lose=async route=>{
      const command=f.m.Request.fromBinary(route.request().postDataBuffer().subarray(5)).command;
      if(!interrupted&&command.oneofKind==='physical'&&command.physical.action.oneofKind==='receiveDevice'){
        interrupted=true;if(accepted)await route.fetch();await route.abort('failed');
      }else await route.continue();
    };
    await page.route('**/omloom.workspace.v1.Workspace/Call',lose);
    try{
      await editor.getByRole('button',{name:'Record received device',exact:true}).click();
      if(!accepted){
        await expect(editor).toContainText('This device was not saved.');
        await expect(editor.getByLabel('Name',{exact:true})).toHaveValue('Automatically checked device');
        const empty=(await f.rpc({oneofKind:'query',query:f.m.Query.create({kinds:[f.m.AssetKind.DEVICE],limit:25})})).page;
        expect(empty.total).toBe(0n);
        await editor.getByRole('button',{name:'Review current receipt',exact:true}).click();
        await editor.getByRole('button',{name:'Record received device',exact:true}).click();
      }
      await expect(planner).toContainText('Unit 1: Automatically checked device');
      await expect(editor.getByRole('button',{name:'Recover receipt result',exact:true})).toHaveCount(0);
      const devices=(await f.rpc({oneofKind:'query',query:f.m.Query.create({kinds:[f.m.AssetKind.DEVICE],limit:25})})).page;
      expect(devices.total).toBe(1n);
    }finally{await page.unroute('**/omloom.workspace.v1.Workspace/Call',lose);}
  });
});

test('a refused receipt preserves a saveable edited plan at the same revision',async({page})=>{
  // Closing an unused creation revision does not change the saved plan. Its
  // concurrent local name edit must remain saveable without reopening the plan.
  await withFieldHost(async url=>{
    const f=await fixture(url),planner=await openPlan(page,url,f);
    await planner.getByRole('button',{name:'Create received device',exact:true}).click();
    const editor=planner.getByRole('region',{name:'Create received device',exact:true});
    let release,started;const gate=new Promise(resolve=>{release=resolve;}),sent=new Promise(resolve=>{started=resolve;});
    const hold=async route=>{
      const command=f.m.Request.fromBinary(route.request().postDataBuffer().subarray(5)).command;
      if(command.oneofKind==='physical'&&command.physical.action.oneofKind==='receiveDevice'){
        started();await gate;await route.abort('failed');
      }else await route.continue();
    };
    await page.route('**/omloom.workspace.v1.Workspace/Call',hold);
    try{
      await editor.getByRole('button',{name:'Record received device',exact:true}).click();await sent;
      await planner.getByLabel('Installation plan name',{exact:true}).fill('Edited while receiving');release();
      await expect(editor).toContainText('This device was not saved.');
      await expect(planner.getByRole('alert').filter({hasText:'Receipt accepted; draft input is retained'})).toHaveCount(0);
      await planner.getByRole('button',{name:'Save installation plan',exact:true}).click();
      await expect(planner).toContainText('Plan saved. Nothing is reserved or installed.');
      const current=(await f.rpc({oneofKind:'physical',physical:f.m.Physical.create({action:{oneofKind:'load',load:{id:f.plan.id}}})})).physical.plan;
      expect(current.name).toBe('Edited while receiving');
      expect(current.materials[0].allocations).toEqual([]);
    }finally{release();await page.unroute('**/omloom.workspace.v1.Workspace/Call',hold);}
  });
});

test('an earlier recovered unit does not hide review of a rejected next device',async({page})=>{
  // Recovery of a different unit must not turn the current form into an already
  // completed receipt or hide its explicit review after an unrelated write.
  await withFieldHost(async url=>{
    const f=await fixture(url),base=f.plan.revision;
    await f.rpc({oneofKind:'physical',physical:f.m.Physical.create({action:{oneofKind:'receiveDevice',receiveDevice:{plan:{id:f.plan.id,revision:f.plan.revision},material:0,unit:0,inventory:{},changes:[{name:'name',value:{oneofKind:'text',text:'Earlier accepted device'}}]}}})});
    const planner=await openPlan(page,url,f);
    await page.evaluate(({plan,revision})=>new Promise((resolve,reject)=>{
      const request=indexedDB.open('omloom-receipt-recovery',1);
      request.onupgradeneeded=()=>request.result.createObjectStore('pending',{autoIncrement:true});
      request.onerror=()=>reject(request.error);
      request.onsuccess=()=>{const db=request.result,tx=db.transaction('pending','readwrite');tx.objectStore('pending').add({plan,revision,material:0,unit:0});tx.oncomplete=()=>{db.close();resolve();};tx.onabort=()=>reject(tx.error);};
    }),{plan:f.plan.id,revision:String(base)});
    await planner.getByRole('button',{name:'Create received device',exact:true}).click();
    const editor=planner.getByRole('region',{name:'Create received device',exact:true});
    await expect(editor).toContainText('Earlier device saved');
    await expect(planner).not.toHaveAttribute('inert','');
    await editor.getByLabel('Name',{exact:true}).fill('Next actual device');
    await expect(editor.getByLabel('Name',{exact:true})).toHaveValue('Next actual device');
    await f.rpc({oneofKind:'physical',physical:f.m.Physical.create({action:{oneofKind:'save',save:{plan:{name:'Concurrent write',materials:[{name:'Demand',quantity:1,kind:f.m.AssetKind.DEVICE}]}}}})});
    await editor.getByRole('button',{name:'Record received device',exact:true}).click();
    await expect(editor).toContainText('Nothing was created by this rejected receipt');
    await expect(editor.getByLabel('Name',{exact:true})).toHaveValue('Next actual device');
    await expect(editor.getByRole('button',{name:'Review current receipt',exact:true})).toBeVisible();
    await editor.getByRole('button',{name:'Review current receipt',exact:true}).click();
    await expect(editor.getByLabel('Name',{exact:true})).toHaveValue('Next actual device');
    await editor.getByRole('button',{name:'Record received device',exact:true}).click();
    await expect(planner).toContainText('Unit 2: Next actual device');
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
