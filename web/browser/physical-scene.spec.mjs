import {test,expect} from '@playwright/test';
import {withFieldHost} from './field-host.mjs';
import {workspaceClient} from './workspace-client.mjs';
import {OrthographicCamera,Vector3} from 'three';
import {mkdtemp,writeFile,rm} from 'node:fs/promises';
import {tmpdir} from 'node:os';
import {join} from 'node:path';

async function interiorRaster(page,png){
  // Fractional CSS scrolling changes the screenshot's one-pixel clip boundary.
  // Compare every interior pixel exactly, including image dimensions, not PNG encoding.
  return page.evaluate(async base64=>{
    const image=await createImageBitmap(new Blob([Uint8Array.from(atob(base64),c=>c.charCodeAt(0))],{type:'image/png'}));
    const width=image.width,height=image.height;
    const canvas=new OffscreenCanvas(width-2,height-2),context=canvas.getContext('2d');
    context.drawImage(image,-1,-1);image.close();
    const pixels=context.getImageData(0,0,width-2,height-2).data;
    const digest=Array.from(new Uint8Array(await crypto.subtle.digest('SHA-256',pixels)));
    return {width,height,digest};
  },png.toString('base64'));
}

async function fixture(url){
  const {m,rpc}=await workspaceClient(url);
  const create=async(kind,name)=>(await rpc({oneofKind:'write',write:m.Write.create({kind,changes:[{name:'name',value:{oneofKind:'text',text:name}}]})})).mutation.targets[0];
  const physical=async action=>{const response=await rpc({oneofKind:'physical',physical:m.Physical.create({action})});expect(response.oneofKind).toBe('physical');return response.physical;};
  const axis=(startUm,lengthUm)=>({startUm,lengthUm});
  const volume=(x,y,z,w,h,d)=>({x:axis(x,w),y:axis(y,h),z:axis(z,d)});
  const rack=await create(m.AssetKind.RACK,'Measured rack');
  const host=await create(m.AssetKind.DEVICE,'Installed server');
  const unknown=await create(m.AssetKind.DEVICE,'Spare server');
  const a=await create(m.AssetKind.PORT,'eth-a'),b=await create(m.AssetKind.PORT,'eth-b');
  const cable=await create(m.AssetKind.CABLE,'Patch cable');
  await physical({oneofKind:'define',define:{definition:{target:rack,basis:'Measured enclosure',volume:volume(0n,0n,0n,600000n,2200000n,1000000n)}}});
  await physical({oneofKind:'define',define:{definition:{target:host,basis:'Measured chassis frame',volume:volume(0n,0n,0n,500000n,200000n,850000n)}}});
  async function actual(steps){const {plan}=await physical({oneofKind:'save',save:{plan:{name:'Recorded inspection',steps}}});await physical({oneofKind:'complete',complete:{plan:{id:plan.id,revision:plan.revision},actualState:true,discrepancy:'Actual inspection; no live connectivity qualification',completedSteps:steps.map((_,i)=>i)}});}
  await actual([{action:{oneofKind:'placement',placement:{item:host,enclosure:rack,volume:volume(50000n,300000n,50000n,500000n,200000n,850000n)}}},{action:{oneofKind:'placement',placement:{item:unknown,enclosure:rack,volume:volume(50000n,700000n,50000n,500000n,200000n,850000n)}}}]);
  for(const port of [a,b])await rpc({oneofKind:'relation',relation:m.Relationship.create({relation:{from:host,to:port,kind:m.RelationKind.HAS_PORT}})});
  const detail=await rpc({oneofKind:'read',read:m.Read.create({target:host})});
  for(const [i,port]of [a,b].entries()){
    const binding=detail.detail.relations.find(r=>r.to?.assetId===port.assetId&&r.kind===m.RelationKind.HAS_PORT);
    expect(binding).toBeDefined();
    await physical({oneofKind:'portAnchor',portAnchor:{bindingId:binding.relationId,expectedBindingRevision:binding.revision,volume:volume(BigInt(40000+i*300000),60000n,0n,40000n,30000n,20000n)}});
  }
  for(const end of ['a','b'])await physical({oneofKind:'cableEnd',cableEnd:{cable,end:{key:end,name:end,basis:'Labelled inspected cable'}}});
  await actual([a,b].map((port,i)=>({action:{oneofKind:'wiring',wiring:{cable,endKey:i?'b':'a',basis:{oneofKind:'expectedEndRevision',expectedEndRevision:1n},intent:m.PhysicalWiringIntent.PRESERVE,after:{oneofKind:'port',port}}}})));
  return{m,rpc,rack,host,unknown,a,b,cable};
}

for(const width of [1440,390])test(`real scoped physical scene and component expansion at ${width}px`,async({page})=>{
  // Real accepted placements and named cable-end effects drive the scene. The
  // camera and selection are reads, while the ordinary resource card remains editable.
  await withFieldHost(async url=>{
    const f=await fixture(url);await page.setViewportSize({width,height:900});
    await page.goto(`${url}/#/?view=2&scope=${f.rack.assetId}&scopeInc=${f.rack.incarnation}`);
    if(width===390)await page.getByRole('button',{name:'More',exact:true}).click();
    await page.getByRole('button',{name:'3D scene',exact:true}).click();
    const scene=page.getByRole('region',{name:'Physical 3D workspace',exact:true});
    await scene.getByRole('button',{name:'Installed server',exact:true}).click();
    await scene.getByRole('button',{name:'Show components',exact:true}).click();
    await expect(scene).toContainText('5 recorded envelopes');
    await expect(scene).toContainText('1 two-end cable diagrams');
    await page.screenshot({path:`test-results/physical-rack-${width}.png`,fullPage:true});
    await scene.getByLabel('3D enclosure',{exact:true}).selectOption(`${f.host.assetId}/${f.host.incarnation}`);
    await expect(scene.getByRole('img',{name:'Physical infrastructure 3D scene'})).toBeVisible();
    await expect(scene).toContainText('1 two-end cable diagrams');
    const canvas=scene.getByRole('img',{name:'Physical infrastructure 3D scene'});
    const rect=await canvas.boundingBox();expect(rect).not.toBeNull();
    // Pick the measured port through the actual GPU scene, not just its list.
    // These coordinates come from the fixture's declared chassis and port envelopes.
    const radius=Math.hypot(.5,.2,.85)*.65,aspect=rect.width/rect.height;
    const camera=new OrthographicCamera(-radius*aspect,radius*aspect,radius,-radius,radius/10000,radius*10);
    const center=new Vector3(.25,-.1,-.425);camera.position.copy(center).add(new Vector3(radius*1.4,radius,radius*1.6));camera.lookAt(center);camera.updateMatrixWorld();
    const position=new Vector3(.06,-.075,-.01).project(camera);
    await canvas.click({position:{x:(position.x+1)*rect.width/2,y:(1-position.y)*rect.height/2}});
    await expect(scene.getByRole('region',{name:'Selected 3D resource'})).toContainText('eth-a');
    await scene.getByRole('button',{name:'eth-a',exact:true}).click();
    await expect(scene.getByRole('region',{name:'Selected 3D resource'})).toContainText('eth-a');
    await scene.getByRole('button',{name:'Rotate right',exact:true}).click();await scene.getByRole('button',{name:'Reset camera',exact:true}).click();
    await page.screenshot({path:`test-results/physical-scene-${width}.png`,fullPage:true});
    expect(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth)).toBe(true);
    await canvas.evaluate(element=>element.getContext('webgl2').getExtension('WEBGL_lose_context').loseContext());
    await expect(scene.getByRole('alert')).toContainText('graphics context was lost');
    await expect(scene.getByRole('button',{name:'eth-a',exact:true})).toBeVisible();
    await scene.getByRole('button',{name:'Retry 3D',exact:true}).click();
    await expect(scene.getByRole('alert')).toHaveCount(0);
    await expect(scene.getByRole('img',{name:'Physical infrastructure 3D scene'})).toBeVisible();
    await scene.getByRole('button',{name:'Open resource',exact:true}).click();
    await expect(page.getByRole('region',{name:'Resource details',exact:true}).getByRole('heading',{name:'eth-a',exact:true})).toBeVisible();
  });
});

test('phone selection has an explicit mode and releases its column when cancelled',async({page})=>{
  // Checkboxes consume no row column until Select is entered. Tablet access
  // stays visible; fine-pointer desktop access remains keyboard reachable.
  await withFieldHost(async url=>{
    const {m,rpc}=await workspaceClient(url);await rpc({oneofKind:'write',write:m.Write.create({kind:m.AssetKind.DEVICE,changes:[{name:'name',value:{oneofKind:'text',text:'Selectable server'}}]})});
    await page.setViewportSize({width:390,height:844});await page.goto(url);
    const checkbox=page.locator('tbody input[type=checkbox]').first();await expect(checkbox).toBeHidden();
    await page.getByRole('button',{name:'Select',exact:true}).click();await expect(checkbox).toBeVisible();await checkbox.check();
    await expect(page.getByText('1 selected on this page',{exact:false})).toBeVisible();
    await page.getByRole('button',{name:'Cancel selection',exact:true}).click();await expect(checkbox).toBeHidden();
    await page.setViewportSize({width:820,height:1000});await expect(checkbox).toBeVisible();await expect(checkbox).toHaveCSS('opacity','1');
    await page.setViewportSize({width:1440,height:900});await page.mouse.move(0,0);await expect(checkbox).toHaveCSS('opacity','0');
    await checkbox.focus();await expect(checkbox).toHaveCSS('opacity','1');
  });
});

test('changing scope during renderer loading creates one current canvas',async({page})=>{
  // Both scope refreshes can await the same lazy module. They must share one
  // owned renderer, without leaving an obsolete canvas or GPU context behind.
  await withFieldHost(async url=>{
    const f=await fixture(url);
    let release,started;
    const gate=new Promise(resolve=>{release=resolve;});
    const requested=new Promise(resolve=>{started=resolve;});
    await page.route(/physical-scene-renderer-.*\.js/,async route=>{started();await gate;await route.continue();});
    try{
      await page.goto(`${url}/#/?view=2&scope=${f.rack.assetId}&scopeInc=${f.rack.incarnation}`);
      await page.getByRole('button',{name:'3D scene',exact:true}).click();await requested;
      await page.goto(`${url}/#/?view=2&scope=${f.host.assetId}&scopeInc=${f.host.incarnation}`);
      const scene=page.getByRole('region',{name:'Physical 3D workspace',exact:true});
      await expect(scene.getByLabel('3D enclosure',{exact:true})).toHaveValue(`${f.host.assetId}/${f.host.incarnation}`);
      await expect(scene.getByRole('button',{name:'eth-a',exact:true})).toBeVisible();
      release();
      await expect(scene.getByRole('status')).toHaveCount(0);
      await expect(scene.getByRole('img',{name:'Physical infrastructure 3D scene'})).toHaveCount(1);
    }finally{release();}
  });
});

test('Reset camera restores the original orthographic zoom',async({page})=>{
  // Reset must restore scale as well as position; otherwise a new enclosure
  // inherits the previous wheel/touch magnification.
  await withFieldHost(async url=>{
    const f=await fixture(url);await page.goto(`${url}/#/?view=2&scope=${f.rack.assetId}&scopeInc=${f.rack.incarnation}`);
    await page.getByRole('button',{name:'3D scene',exact:true}).click();
    const scene=page.getByRole('region',{name:'Physical 3D workspace',exact:true});
    const canvas=scene.getByRole('img',{name:'Physical infrastructure 3D scene'});
    await expect(canvas).toBeVisible();
    const before=await canvas.screenshot({path:test.info().outputPath('reset-before.png')});
    await canvas.hover();await page.mouse.wheel(0,-400);
    await page.evaluate(()=>new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(resolve))));
    const baseline=await interiorRaster(page,before);
    expect(await interiorRaster(page,await canvas.screenshot())).not.toEqual(baseline);
    await scene.getByRole('button',{name:'Reset camera',exact:true}).click();
    const after=await canvas.screenshot({path:test.info().outputPath('reset-after.png')});
    await test.info().attach('reset-before',{body:before,contentType:'image/png'});
    await test.info().attach('reset-after',{body:after,contentType:'image/png'});
    expect(await interiorRaster(page,after)).toEqual(baseline);
  });
});

test('name-only physical children cannot poison component expansion',async({page})=>{
  // Real store grants allow the port label and relation but deny its physical
  // field. Expanding the server must retain the authorized enclosure scene.
  const dir=await mkdtemp(join(tmpdir(),'omloom-scene-grants-')),grants=join(dir,'grants.json');
  await writeFile(grants,JSON.stringify([
    {selection:{scope:'all'},read:{mode:'selected',fields:['name','relations']},write:{mode:'all'},writable:true},
    {selection:{scope:'kinds',kinds:['ASSET_KIND_RACK','ASSET_KIND_DEVICE']},read:{mode:'all'},write:{mode:'all'},writable:true},
  ]));
  try{await withFieldHost(async url=>{
    const {m,rpc}=await workspaceClient(url);
    const create=async(kind,name)=>(await rpc({oneofKind:'write',write:m.Write.create({kind,changes:[{name:'name',value:{oneofKind:'text',text:name}}]})})).mutation.targets[0];
    const rack=await create(m.AssetKind.RACK,'Permitted rack'),host=await create(m.AssetKind.DEVICE,'Permitted server'),port=await create(m.AssetKind.PORT,'Name-only port');
    await rpc({oneofKind:'relation',relation:m.Relationship.create({relation:{from:host,to:port,kind:m.RelationKind.HAS_PORT}})});
    const axis={startUm:0n,lengthUm:1000000n},volume={x:axis,y:axis,z:axis};
    const physical=async action=>(await rpc({oneofKind:'physical',physical:m.Physical.create({action})})).physical;
    await physical({oneofKind:'define',define:{definition:{target:rack,basis:'Measured rack',volume}}});
    const {plan}=await physical({oneofKind:'save',save:{plan:{name:'Actual position',steps:[{action:{oneofKind:'placement',placement:{item:host,enclosure:rack,volume}}}]}}});
    await physical({oneofKind:'complete',complete:{plan:{id:plan.id,revision:plan.revision},actualState:true,discrepancy:'Inspected installation',completedSteps:[0]}});
    await page.goto(`${url}/#/?view=2&scope=${rack.assetId}&scopeInc=${rack.incarnation}`);
    await page.getByRole('button',{name:'3D scene',exact:true}).click();
    const scene=page.getByRole('region',{name:'Physical 3D workspace',exact:true});
    await scene.getByRole('button',{name:'Permitted server',exact:true}).click();
    await scene.getByRole('button',{name:'Show components',exact:true}).click();
    await expect(scene.getByRole('status')).toHaveCount(0);
    await expect(scene.getByRole('alert')).toHaveCount(0);
    await expect(scene.getByRole('button',{name:'Permitted rack',exact:true})).toBeVisible();
    await expect(scene).not.toContainText('Name-only port');
  },{command:['target/debug/omloom-dev','--grants',grants]});}finally{await rm(dir,{recursive:true,force:true});}
});
