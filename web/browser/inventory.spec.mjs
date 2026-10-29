import { test, expect } from '@playwright/test';
import { spawn, execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { mkdtemp, rm, writeFile, readFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { createServer } from 'node:http';

async function host(directory, fixture = 'operator', grants) {
  const args = ['--data', directory, '--ui', 'dist/app', '--fixture', fixture];
  if (grants) args.push('--grants', grants);
  const child = spawn('target/debug/omloom-dev', args, { stdio: ['ignore', 'pipe', 'pipe'] });
  const url = await new Promise((resolve, reject) => {
    let output = '';
    const timer = setTimeout(() => { child.kill('SIGINT'); reject(Error(`Host startup timed out: ${output}`)); }, 20_000);
    child.stdout.on('data', chunk => { output += chunk; const match = output.match(/http:\/\/127\.0\.0\.1:\d+/); if (match) { clearTimeout(timer); resolve(match[0]); } });
    child.stderr.on('data', chunk => { output += chunk; });
    child.once('exit', code => { clearTimeout(timer); reject(Error(`Host exited ${code}: ${output}`)); });
  });
  return { url, async crash() { const exit=new Promise(resolve=>child.once('exit',resolve));child.kill('SIGKILL');await exit; }, async stop() { if (child.exitCode !== null || child.signalCode !== null) return; const exit = new Promise(resolve => child.once('exit', resolve)); child.kill('SIGINT'); await exit; } };
}
async function create(page, name, kind = 'DEVICE') {
  // These fixtures create independent roots. Child creation has its own scoped journey.
  const close = page.getByRole('button', { name: 'Close resource details', exact: true });
  if (await close.isVisible()) { await close.click(); await expect(page.getByRole('region', { name: 'Resource details' })).toBeHidden(); }
  await page.getByRole('button', { name: '+ Create resource', exact: true }).click();
  const dialog = page.getByRole('dialog');
  await dialog.getByLabel('Resource kind', { exact: true }).selectOption({ label: kind.toLowerCase().replaceAll('_', ' ') }, { timeout: 5000 });
  await dialog.getByLabel('Name', { exact: true }).fill(name, { timeout: 5000 });
  await dialog.getByRole('button', { name: 'Save resource', exact: true }).click();
  await expect(dialog).toBeHidden();
  await expect(page.getByRole('region', { name: 'Resource details' }).getByRole('heading', { name, exact: true })).toBeVisible();
  await expect(page.getByRole('region', { name: 'Resource details' })).toBeVisible();
  const hash = new URL(page.url()).hash;
  const params = new URLSearchParams(hash.slice(hash.indexOf('?') + 1));
  return { asset_id: params.get('asset'), incarnation: params.get('incarnation') };
}

async function workspaceClient(url) {
  const { buildSync } = await import('esbuild');
  const { GrpcWebFetchTransport } = await import('@protobuf-ts/grpcweb-transport');
  const built = buildSync({ stdin: { contents: 'export * from "./web/src/generated/omloom/v1/workspace"; export {AssetKind, RelationKind, EffectOutcome, OperationState} from "./web/src/generated/omloom/v1/contracts";', resolveDir: process.cwd() }, bundle: true, write: false, format: 'esm', platform: 'node' });
  const m = await import('data:text/javascript;base64,' + Buffer.from(built.outputFiles[0].contents).toString('base64'));
  const transport = new GrpcWebFetchTransport({ baseUrl: url, format: 'binary' });
  async function rpc(command, focus) {
    const q = await transport.unary(m.Workspace.methods[0], m.Request.create({ command: { oneofKind: 'query', query: m.Query.create({ limit: 1 }) } }), transport.mergeOptions());
    return (await transport.unary(m.Workspace.methods[0], m.Request.create({ command, working: { expected: q.response.result.page.revision, focus } }), transport.mergeOptions())).response.result;
  }
  return { m, rpc };
}

test('host report adoption is selective and scoped on desktop and phone', async ({ page }) => {
  // Report-local keys stay outside inventory until explicit review. Accepted
  // components remain inside their parent rather than flattening global lists.
  const dir=await mkdtemp(join(tmpdir(),'omloom-host-import-'));const server=await host(dir);
  try {
    const {m,rpc}=await workspaceClient(server.url);
    for(const width of [1440,390]) {
      await page.setViewportSize({width,height:900});await page.goto(server.url);
      if(width===390)await page.getByRole('button',{name:'More',exact:true}).click();
      await page.getByRole('button',{name:'Import host report',exact:true}).click();
      const dialog=page.getByRole('dialog');
      const field=(text)=>({name:'name',value:{oneofKind:'text',text}});
      const hardwareKey=width===390?'constructor':'hardware';
      const report=m.HostReport.create({version:1,platform:'macos',sourceBasis:String(width).padEnd(64,'a'),bootBasis:'b'.repeat(64),observedAt:100n,findings:[
        {key:hardwareKey,kind:m.AssetKind.DEVICE,fields:[field(`Collected host ${width}`)]},
        {key:'installation',kind:m.AssetKind.SOFTWARE_INSTALLATION,parent:hardwareKey,fields:[field(`Collected OS ${width}`)]},
        {key:'interface/en0',kind:m.AssetKind.INTERFACE,parent:'installation',fields:[field(`Collected NIC ${width}`)],addresses:[{address:'192.0.2.7',prefixLength:24,addressStability:undefined,stability:m.AddressStability.STABLE,state:m.AddressState.PREFERRED}],addressesComplete:true},
      ],coverage:[{module:'interfaces',status:'verified',detail:'Synthetic current interface fixture'}]});
      const buffer=Buffer.from(m.Request.toBinary(m.Request.create({command:{oneofKind:'hostImport',hostImport:m.HostImport.create({report})}})));
      await dialog.getByLabel('Host report file').setInputFiles({name:'host.omloom',mimeType:'application/octet-stream',buffer});
      await dialog.getByLabel('Source name',{exact:true}).fill(`Native source ${width}`);
      await expect(dialog.getByText('interfaces: verified',{exact:true})).toBeVisible();
      await dialog.getByRole('checkbox').nth(0).check();await dialog.getByRole('checkbox').nth(1).check();
      await dialog.getByRole('button',{name:'Prepare host preview',exact:true}).click();
      await expect(dialog.getByRole('heading',{name:'Review 2 selected resources',exact:true})).toBeVisible();
      const before=await rpc({oneofKind:'query',query:m.Query.create({kinds:[m.AssetKind.DEVICE],search:`Collected host ${width}`,limit:25})});
      expect(before.page.total).toBe(0n);
      await dialog.getByRole('button',{name:'Accept selected observations',exact:true}).click();await expect(dialog).toBeHidden();
      await expect(page.getByRole('button',{name:`Collected host ${width}`,exact:true})).toBeVisible();
      await expect(page.getByRole('button',{name:`Collected NIC ${width}`,exact:true})).toHaveCount(0);
      await page.getByRole('button',{name:`Collected host ${width}`,exact:true}).click();
      await page.getByRole('button',{name:'Use as scope',exact:true}).click();
      await expect(page.getByRole('button',{name:`Collected OS ${width}`,exact:true})).toBeVisible();
      expect(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth)).toBe(true);
    }
  }finally{await server.stop();await rm(dir,{recursive:true,force:true});}
});

test('VM placement uses desktop drag and phone selection without changing guest identity or IP', async ({page})=>{
  // Real host/store/UI: record an external migration, then independently update
  // current guest addresses. No hypervisor call or migration result is mocked.
  const dir=await mkdtemp(join(tmpdir(),'omloom-vm-placement-'));const server=await host(dir);
  try {
    const {m,rpc}=await workspaceClient(server.url);
    async function resource(kind,name,parent,extra=[]){const result=await rpc({oneofKind:'write',write:m.Write.create({kind,parent,changes:[{name:'name',value:{oneofKind:'text',text:name}},...extra]})});return result.mutation.targets[0];}
    const role={name:'virtualization_role',value:{oneofKind:'text',text:'VIRTUALIZATION_ROLE_HYPERVISOR'}};
    const chassisA=await resource(m.AssetKind.DEVICE,'Compute chassis A');
    const chassisB=await resource(m.AssetKind.DEVICE,'Compute chassis B');
    const a=await resource(m.AssetKind.SOFTWARE_INSTALLATION,'Migration host A',chassisA,[role]);
    const b=await resource(m.AssetKind.SOFTWARE_INSTALLATION,'Migration host B',chassisB,[role]);
    const vm=await resource(m.AssetKind.VIRTUAL_MACHINE,'Retained guest');
    const os=await resource(m.AssetKind.SOFTWARE_INSTALLATION,'Retained guest OS',vm);
    const nic=await resource(m.AssetKind.INTERFACE,'Guest eth0',os);
    const service=await resource(m.AssetKind.SERVICE,'Guest listener',os);
    await rpc({oneofKind:'relation',relation:m.Relationship.create({relation:{from:vm,to:a,kind:m.RelationKind.RUNS_ON}})});
    async function address(value,time){await rpc({oneofKind:'interfaceAddresses',interfaceAddresses:m.InterfaceAddresses.create({interface:nic,source:'Guest report',observedAt:BigInt(time),complete:true,addresses:[{address:value,stability:m.AddressStability.STABLE,state:m.AddressState.PREFERRED}]})});}
    await address('192.0.2.40',50);
    const originalChildren=[];
    for(const target of [os,nic,service]) originalChildren.push((await rpc({oneofKind:'read',read:m.Read.create({target})})).detail.item);
    let expectedIp='192.0.2.40';
    for(const [width,from,to,name] of [[1440,a,b,'Migration host B'],[390,b,a,'Migration host A']]) {
      await page.setViewportSize({width,height:900});
      await page.goto(`${server.url}/#/?view=3&kind=${m.AssetKind.DEVICE}&scope=${from.assetId}&scopeInc=${from.incarnation}&asset=${vm.assetId}&incarnation=${vm.incarnation}`);
      await page.getByRole('button',{name:'Change hypervisor after migration',exact:true}).click();
      const dialog=page.getByRole('dialog',{name:'Record VM placement'});
      await expect(dialog.getByText('Current host:',{exact:false})).toBeVisible();
      // Widening the selected context is explicit, rather than a hidden search fallback.
      await dialog.getByRole('button',{name:'Choose from all permitted infrastructure',exact:true}).click();
      const destination=dialog.getByRole('article',{name:`Destination ${name}`,exact:true});
      await expect(destination).toBeVisible();
      await expect(destination).toContainText(width===1440?'Compute chassis B':'Compute chassis A');
      const before=(await rpc({oneofKind:'query',query:m.Query.create({limit:1})})).page.revision;
      if(width===1440) await dialog.getByRole('article',{name:'VM to relocate',exact:true}).dragTo(destination);
      else await destination.getByRole('button',{name:`Choose ${name}`,exact:true}).click();
      await expect(dialog.getByRole('button',{name:'Record and view VM',exact:true})).toBeVisible();
      await dialog.getByRole('button',{name:'Back to destinations',exact:true}).click();
      await destination.getByRole('button',{name:`Choose ${name}`,exact:true}).click();
      expect((await rpc({oneofKind:'query',query:m.Query.create({limit:1})})).page.revision).toBe(before);
      expect(await dialog.evaluate(el=>el.scrollWidth<=el.clientWidth)).toBe(true);
      await page.screenshot({path:`test-results/vm-placement-${width}.png`,fullPage:true});
      await dialog.getByRole('button',{name:'Record and view VM',exact:true}).click();
      await expect(dialog).toBeHidden();
      await expect(page.getByRole('heading',{name:'Retained guest',exact:true})).toBeVisible();
      const current=(await rpc({oneofKind:'prepareVmPlacement',prepareVmPlacement:m.PrepareVmPlacement.create({vm})})).vmPlacementReview;
      expect(current.vm.target).toEqual(vm);expect(current.currentHost.target).toEqual(to);
      const addresses=(await rpc({oneofKind:'read',read:m.Read.create({target:nic})})).detail.currentAddresses;
      expect(addresses).toHaveLength(1);expect(addresses[0].observations[0].value.address).toBe(expectedIp);
      for(const [index,target] of [os,nic,service].entries()) expect((await rpc({oneofKind:'read',read:m.Read.create({target})})).detail.item).toEqual(originalChildren[index]);
      expect(await page.evaluate(()=>document.documentElement.scrollWidth<=window.innerWidth)).toBe(true);
      await page.getByRole('button',{name:'Close resource details',exact:true}).click();
      await expect(page.getByRole('button',{name:'Retained guest',exact:true})).toBeVisible();
      if(width===1440){await address('192.0.2.41',60);expectedIp='192.0.2.41';}
    }
  }finally{await server.stop();await rm(dir,{recursive:true,force:true});}
});

test('accepted correction and exact retry survive an abrupt host exit', async ({page})=>{
  // Process crash after acknowledgement, not a claim about device power loss.
  const dir=await mkdtemp(join(tmpdir(),'omloom-correction-crash-'));let server=await host(dir);
  try {
    let {m,rpc}=await workspaceClient(server.url);const targets=[];
    for(const name of ['Crash retained','Crash duplicate']){const saved=await rpc({oneofKind:'write',write:m.Write.create({kind:m.AssetKind.DEVICE,changes:[{name:'name',value:{oneofKind:'text',text:name}}]})});targets.push(saved.mutation.targets[0]);}
    const prepared=await rpc({oneofKind:'correctIdentity',correctIdentity:m.IdentityCorrection.create({decision:m.IdentityDecision.DISTINCT_INSTALLATIONS,mappings:[{duplicate:targets[1],retained:targets[0],fields:[{name:'name',useDuplicate:false}]}]})});
    const accepted=await rpc({oneofKind:'correctIdentity',correctIdentity:m.IdentityCorrection.create({preview:prepared.identityCorrection.id})});
    await server.crash();server=await host(dir);({m,rpc}=await workspaceClient(server.url));
    const retry=await rpc({oneofKind:'correctIdentity',correctIdentity:m.IdentityCorrection.create({preview:prepared.identityCorrection.id})});expect(retry.identityCorrected).toEqual(accepted.identityCorrected);
    const alias=await rpc({oneofKind:'resolveIdentity',resolveIdentity:m.ResolveIdentity.create({original:targets[1]})});expect(alias.resolvedIdentity.canonical.target).toEqual(targets[0]);
    await rpc({oneofKind:'correctIdentity',correctIdentity:m.IdentityCorrection.create({undo:accepted.identityCorrected.id})});
    await page.goto(server.url);await expect(page.getByRole('button',{name:'Crash duplicate',exact:true})).toBeVisible();await expect(page.getByRole('button',{name:'Crash retained',exact:true})).toBeVisible();
  }finally{await server.stop();await rm(dir,{recursive:true,force:true});}
});

test('reviewed identity correction keeps both boots and reverses on desktop and phone', async ({ page }) => {
  // Real disk/API/UI: a hardware correction keeps separate installations and
  // their interfaces. Back preserves the reviewed draft; reversal restores it.
  const dir=await mkdtemp(join(tmpdir(),'omloom-correction-'));const server=await host(dir);
  try {
    const {m,rpc}=await workspaceClient(server.url);
    for(const width of [1440,390]) {
      const prefix=`Correction ${width}`;const targets=[];
      for(const side of ['left','right']) {
        const report=m.HostReport.create({version:1,platform:'linux',sourceBasis:`${width}${side==='left'?'1':'2'}`.padEnd(64,'a'),bootBasis:'b'.repeat(64),observedAt:100n,findings:[
          {key:'hardware',kind:m.AssetKind.DEVICE,fields:[{name:'name',value:{oneofKind:'text',text:`${prefix} ${side}`}}],identity:[{kind:m.HostIdentityKind.HARDWARE_UUID,namespace:'smbios/system-uuid',digest:'a'.repeat(64),quality:m.HostIdentityQuality.REPORTED}]},
          {key:'installation',parent:'hardware',kind:m.AssetKind.SOFTWARE_INSTALLATION,fields:[{name:'name',value:{oneofKind:'text',text:`Boot ${side}`}}]},
          {key:'interface/en0',parent:'installation',kind:m.AssetKind.INTERFACE,fields:[{name:'name',value:{oneofKind:'text',text:'en0'}}],addresses:[{address:side==='left'?'192.0.2.10':'192.0.2.11',stability:m.AddressStability.STABLE,state:m.AddressState.PREFERRED}],addressesComplete:true}
        ]});
        const prepared=await rpc({oneofKind:'hostImport',hostImport:m.HostImport.create({source:`Boot ${side} ${width}`,report,selection:report.findings.map(v=>({key:v.key}))})});
        const accepted=await rpc({oneofKind:'hostImport',hostImport:m.HostImport.create({preview:prepared.preview.id})});targets.push(accepted.mutation.targets);
      }
      await page.setViewportSize({width,height:900});await page.goto(`${server.url}/#/?view=2&asset=${targets[0][0].assetId}&incarnation=${targets[0][0].incarnation}`);
      await page.getByRole('button',{name:'Find possible duplicates in enclosing selection',exact:true}).click();
      await page.getByRole('region',{name:'Identity candidates',exact:true}).locator('li').filter({hasText:`${prefix} right`}).getByRole('button',{name:'Compare with candidate',exact:true}).click();const dialog=page.getByRole('dialog');
      await dialog.getByRole('button',{name:'Plan identity correction',exact:true}).click();
      await dialog.getByRole('radio',{name:'Same hardware, different OS installations',exact:true}).check();
      await dialog.getByRole('button',{name:'Continue',exact:true}).click();
      await dialog.getByRole('combobox',{name:'Retained identity',exact:true}).selectOption('left',{timeout:5000});
      await dialog.getByRole('button',{name:'Keep all retained field values',exact:true}).click();
      await dialog.getByRole('button',{name:'Prepare correction preview',exact:true}).click();
      await expect(dialog.getByRole('heading',{name:'Review identity correction',exact:true})).toBeVisible();
      await dialog.getByRole('button',{name:'Back',exact:true}).click();
      await expect(dialog.getByRole('combobox',{name:'Retained identity',exact:true})).toHaveValue('left');
      await dialog.getByRole('button',{name:'Prepare correction preview',exact:true}).click();
      await dialog.getByRole('button',{name:'Accept identity correction',exact:true}).click();
      await expect(dialog.getByRole('heading',{name:'Correction applied',exact:true})).toBeVisible();
      const query=await rpc({oneofKind:'query',query:m.Query.create({view:m.View.HARDWARE,search:prefix,limit:25})});expect(query.page.total).toBe(1n);
      for(const [index,side] of ['left','right'].entries()) {
        const detail=await rpc({oneofKind:'read',read:m.Read.create({target:targets[index][1]})});
        expect(detail.detail.relations.some(v=>v.kind===m.RelationKind.CONTAINS&&v.from?.assetId===targets[0][0].assetId&&v.to?.assetId===targets[index][1].assetId)).toBe(true);
        const iface=await rpc({oneofKind:'read',read:m.Read.create({target:targets[index][2]})});expect(iface.detail.currentAddresses[0].observations[0].value.address).toBe(side==='left'?'192.0.2.10':'192.0.2.11');
      }
      expect(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth)).toBe(true);
      await dialog.getByRole('button',{name:'Reverse this correction',exact:true}).click();
      await expect(dialog.getByRole('heading',{name:'Correction reversed',exact:true})).toBeVisible();
      await page.screenshot({path:`test-results/identity-correction-${width}.png`,fullPage:true});
      await dialog.getByRole('button',{name:'Close',exact:true}).click();
      const restored=await rpc({oneofKind:'query',query:m.Query.create({view:m.View.HARDWARE,search:prefix,limit:25})});expect(restored.page.total).toBe(2n);
    }
  } finally {await server.stop();await rm(dir,{recursive:true,force:true});}
});

test('duplicate installation review maps only selected layers on desktop and phone',async({page})=>{
  // Each selected layer has its own field decision. Equal native labels keep
  // separate source bindings, and reversal restores both source namespaces.
  const dir=await mkdtemp(join(tmpdir(),'omloom-layer-correction-'));const server=await host(dir);
  try {
    const {m,rpc}=await workspaceClient(server.url);
    for(const width of [1440,390]) {
      const prefix=`Layer ${width}`;const targets=[];
      for(const side of ['left','right']) {
        const field=text=>({name:'name',value:{oneofKind:'text',text}});
        const report=m.HostReport.create({version:1,platform:'linux',sourceBasis:`${width}${side==='left'?'1':'2'}`.padEnd(64,'a'),bootBasis:'b'.repeat(64),observedAt:100n,findings:[
          {key:'hardware',kind:m.AssetKind.DEVICE,fields:[field(`${prefix} ${side}`)]},
          {key:'installation',parent:'hardware',kind:m.AssetKind.SOFTWARE_INSTALLATION,fields:[field(`${prefix} OS ${side}`)]},
          {key:'interface/en0',parent:'installation',kind:m.AssetKind.INTERFACE,fields:[field(`${prefix} NIC ${side}`)],addresses:[{address:side==='left'?'192.0.2.10':'192.0.2.11',stability:m.AddressStability.STABLE,state:m.AddressState.PREFERRED}],addressesComplete:true}
        ]});
        const prepared=await rpc({oneofKind:'hostImport',hostImport:m.HostImport.create({source:`Shared native label ${width}`,report,selection:report.findings.map(v=>({key:v.key}))})});
        const accepted=await rpc({oneofKind:'hostImport',hostImport:m.HostImport.create({preview:prepared.preview.id})});targets.push(accepted.mutation.targets);
      }
      await page.setViewportSize({width,height:900});await page.goto(`${server.url}/#/?view=2&search=${encodeURIComponent(prefix)}`);
      for(const side of ['left','right'])await page.getByRole('checkbox',{name:`Select ${prefix} ${side}`,exact:true}).check();
      await page.getByRole('button',{name:'Compare identity',exact:true}).click();const dialog=page.getByRole('dialog');
      await dialog.getByRole('button',{name:'Plan identity correction',exact:true}).click();await dialog.getByRole('radio',{name:'Duplicate records of the same hardware and installation',exact:true}).check();await dialog.getByRole('button',{name:'Continue',exact:true}).click();
      await dialog.getByRole('combobox',{name:'Retained identity',exact:true}).selectOption('left');await dialog.getByRole('button',{name:'Keep all retained field values',exact:true}).click();
      await dialog.getByText('Add an installation or interface mapping',{exact:true}).click();
      for(const [index,kind,name] of [[2,m.AssetKind.SOFTWARE_INSTALLATION,'OS'],[3,m.AssetKind.INTERFACE,'NIC']]) {
        await dialog.getByRole('combobox',{name:'Layer',exact:true}).selectOption(String(kind));await dialog.getByRole('button',{name:'Find layer resources',exact:true}).click();
        for(const [side,action] of [['right','Use as duplicate'],['left','Use as retained']])await dialog.locator('li').filter({hasText:`${prefix} ${name} ${side}`}).getByRole('button',{name:action,exact:true}).click();
        await dialog.getByRole('button',{name:'Add layer mapping',exact:true}).click();
        await dialog.getByRole('region',{name:`Layer mapping ${index}`,exact:true}).getByRole('button',{name:'Keep all retained field values',exact:true}).click();
      }
      await dialog.getByRole('button',{name:'Prepare correction preview',exact:true}).click();await expect(dialog.getByRole('heading',{name:'Review identity correction',exact:true})).toBeVisible();await dialog.getByRole('button',{name:'Accept identity correction',exact:true}).click();await expect(dialog.getByRole('heading',{name:'Correction applied',exact:true})).toBeVisible();
      const current=await rpc({oneofKind:'read',read:m.Read.create({target:targets[0][2]})});expect(current.detail.currentAddresses).toHaveLength(2);expect(new Set(current.detail.currentAddresses.map(v=>v.sourceId)).size).toBe(2);
      expect(current.detail.currentAddresses.flatMap(v=>v.observations.map(v=>v.value.address)).sort()).toEqual(['192.0.2.10','192.0.2.11']);
      expect(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth)).toBe(true);
      await dialog.getByRole('button',{name:'Reverse this correction',exact:true}).click();await expect(dialog.getByRole('heading',{name:'Correction reversed',exact:true})).toBeVisible();await dialog.getByRole('button',{name:'Close',exact:true}).click();
      for(const target of targets){const restored=await rpc({oneofKind:'read',read:m.Read.create({target:target[2]})});expect(restored.detail.currentAddresses).toHaveLength(1);}
    }
  }finally{await server.stop();await rm(dir,{recursive:true,force:true});}
});

test('identity comparison keeps two selected machines distinct on desktop and phone', async ({ page }) => {
  // Equal unauthenticated hardware claims support a pair review. Opening and
  // leaving that review must not merge identities, widen scope or write state.
  const dir = await mkdtemp(join(tmpdir(), 'omloom-identity-pair-'));
  const server = await host(dir);
  try {
    const { m, rpc } = await workspaceClient(server.url);
    for (const width of [1440, 390]) {
      const prefix = `Identity pair ${width}`;
      for (const side of ['left', 'right']) {
        const report = m.HostReport.create({ version: 1, platform: 'linux', sourceBasis: `${width}${side === 'left' ? '1' : '2'}`.padEnd(64, 'a'), bootBasis: 'b'.repeat(64), observedAt: 100n,
          findings: [{ key: 'hardware', kind: m.AssetKind.DEVICE, fields: [{ name: 'name', value: { oneofKind: 'text', text: `${prefix} ${side}` } }],
            identity: [{ kind: m.HostIdentityKind.HARDWARE_UUID, namespace: 'smbios/system-uuid', digest: 'a'.repeat(64), quality: m.HostIdentityQuality.REPORTED }] }] });
        const prepared = await rpc({ oneofKind: 'hostImport', hostImport: m.HostImport.create({ source: `${width} ${side}`, report, selection: [{ key: 'hardware' }] }) });
        expect(prepared.oneofKind).toBe('preview');
        await rpc({ oneofKind: 'hostImport', hostImport: m.HostImport.create({ preview: prepared.preview.id }) });
      }
      const query = { oneofKind: 'query', query: m.Query.create({ search: prefix, limit: 25 }) };
      const before = await rpc(query);
      expect(before.page.total).toBe(2n);
      await page.setViewportSize({ width, height: 900 });
      await page.goto(`${server.url}/#/?view=2&search=${encodeURIComponent(prefix)}`);
      await page.getByRole('checkbox', { name: `Select ${prefix} left`, exact: true }).check();
      await page.getByRole('checkbox', { name: `Select ${prefix} right`, exact: true }).check();
      await page.getByRole('button', { name: 'Compare identity', exact: true }).click();
      const dialog = page.getByRole('dialog');
      await expect(dialog.getByRole('heading', { name: `${prefix} left`, exact: true })).toBeVisible();
      await expect(dialog.getByRole('heading', { name: `${prefix} right`, exact: true })).toBeVisible();
      await expect(dialog.getByText('Some identifiers match within the same layer and namespace.', { exact: false })).toBeVisible();
      await expect(dialog.getByText('smbios/system-uuid · reported', { exact: true })).toHaveCount(2);
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
      await dialog.getByRole('button', { name: 'Back', exact: true }).click();
      await expect(dialog).toBeHidden();
      await expect(page.getByRole('checkbox', { name: `Select ${prefix} left`, exact: true })).toBeChecked();
      const after = await rpc(query);
      expect(after.page.total).toBe(2n);
      expect(after.page.revision).toBe(before.page.revision);
    }
  } finally { await server.stop(); await rm(dir, { recursive: true, force: true }); }
});

test('host address replacement is reviewed without adopting another identity on desktop and phone', async ({ page }) => {
  // The import review shows only a selected source's changes. Accepting the
  // new address reuses all three existing identities instead of creating hosts.
  const dir = await mkdtemp(join(tmpdir(), 'omloom-address-delta-'));
  const server = await host(dir);
  try {
    const { m, rpc } = await workspaceClient(server.url);
    for (const width of [1440, 390]) {
      const source = `Address source ${width}`;
      const field = text => ({ name: 'name', value: { oneofKind: 'text', text } });
      const address = value => m.CurrentAddress.create({ address: value, prefixLength: 24, stability: m.AddressStability.STABLE, state: m.AddressState.PREFERRED });
      const report = m.HostReport.create({ version: 1, platform: 'linux', sourceBasis: `${width}`.padEnd(64, 'a'), bootBasis: 'b'.repeat(64), observedAt: 100n, findings: [
        { key: 'hardware', kind: m.AssetKind.DEVICE, fields: [field(`Address host ${width}`)] },
        { key: 'installation', kind: m.AssetKind.SOFTWARE_INSTALLATION, parent: 'hardware', fields: [field(`Address OS ${width}`)] },
        { key: 'interface', kind: m.AssetKind.INTERFACE, parent: 'installation', fields: [field(`Address NIC ${width}`)], addresses: [address('192.0.2.7')], addressesComplete: true },
      ] });
      const prepared = await rpc({ oneofKind: 'hostImport', hostImport: m.HostImport.create({ source, report, selection: report.findings.map(v => ({ key: v.key })) }) });
      const accepted = await rpc({ oneofKind: 'hostImport', hostImport: m.HostImport.create({ preview: prepared.preview.id }) });
      report.observedAt = 200n;
      report.findings[2].addresses = [address('192.0.2.9')];
      const buffer = Buffer.from(m.Request.toBinary(m.Request.create({ command: { oneofKind: 'hostImport', hostImport: m.HostImport.create({ report }) } })));
      await page.setViewportSize({ width, height: 900 }); await page.goto(server.url);
      if (width === 390) await page.getByRole('button', { name: 'More', exact: true }).click();
      await page.getByRole('button', { name: 'Import host report', exact: true }).click();
      const dialog = page.getByRole('dialog');
      await dialog.getByLabel('Host report file').setInputFiles({ name: 'address-change.omloom', mimeType: 'application/octet-stream', buffer });
      await dialog.getByLabel('Source name', { exact: true }).fill(source);
      for (let i = 0; i < 3; i++) await dialog.getByRole('checkbox').nth(i).check();
      await dialog.getByRole('button', { name: 'Prepare host preview', exact: true }).click();
      await expect(dialog.getByText('Add 192.0.2.9/24 (preferred)', { exact: true })).toBeVisible();
      await expect(dialog.getByText('Remove 192.0.2.7/24 (preferred) from this source', { exact: true })).toBeVisible();
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
      await dialog.getByRole('button', { name: 'Accept selected observations', exact: true }).click();
      await expect(dialog).toBeHidden();
      const current = await rpc({ oneofKind: 'read', read: m.Read.create({ target: accepted.mutation.targets[2] }) });
      expect(current.detail.currentAddresses[0].observations.map(v => v.value.address)).toEqual(['192.0.2.9']);
      const hosts = await rpc({ oneofKind: 'query', query: m.Query.create({ search: `Address host ${width}`, limit: 25 }) });
      expect(hosts.page.total).toBe(1n);
      expect(hosts.page.items[0].target).toEqual(accepted.mutation.targets[0]);
    }
  } finally { await server.stop(); await rm(dir, { recursive: true, force: true }); }
});

test('execution host reports match the current VM scope on desktop and phone', async ({ page }) => {
  // Guest collection cannot invent a physical server or rename the accepted VM.
  const dir=await mkdtemp(join(tmpdir(),'omloom-host-vm-'));const server=await host(dir);
  try {
    const {m,rpc}=await workspaceClient(server.url);
    for(const width of [1440,390]) {
      await page.setViewportSize({width,height:900});await page.goto(server.url);
      const target=await create(page,`Known VM ${width}`,'VIRTUAL_MACHINE');
      await page.getByRole('button',{name:'Use as scope',exact:true}).click();
      if(width===390)await page.getByRole('button',{name:'More',exact:true}).click();
      await page.getByRole('button',{name:'Import host report',exact:true}).click();
      const dialog=page.getByRole('dialog');
      const field=text=>({name:'name',value:{oneofKind:'text',text}});
      const report=m.HostReport.create({version:1,platform:'linux',sourceBasis:String(width).padEnd(64,'a'),bootBasis:'b'.repeat(64),observedAt:100n,findings:[
        {key:'hardware',kind:m.AssetKind.UNSPECIFIED,fields:[field('Guest hostname')]},
        {key:'installation',kind:m.AssetKind.SOFTWARE_INSTALLATION,parent:'hardware',fields:[field(`Guest OS ${width}`)]},
      ]});
      const buffer=Buffer.from(m.Request.toBinary(m.Request.create({command:{oneofKind:'hostImport',hostImport:m.HostImport.create({report})}})));
      await dialog.getByLabel('Host report file').setInputFiles({name:'host.omloom',mimeType:'application/octet-stream',buffer});
      await dialog.getByLabel('Source name',{exact:true}).fill(`VM source ${width}`);
      await dialog.getByRole('button',{name:'Use current device or VM scope',exact:true}).click();
      await expect(dialog.getByText(`Existing match: Known VM ${width}`,{exact:true})).toBeVisible();
      await dialog.getByRole('checkbox').nth(0).check();await dialog.getByRole('checkbox').nth(1).check();
      await dialog.getByRole('button',{name:'Prepare host preview',exact:true}).click();
      await expect(dialog.getByRole('heading',{name:'Review 2 selected resources',exact:true})).toBeVisible();
      await dialog.getByRole('button',{name:'Accept selected observations',exact:true}).click();await expect(dialog).toBeHidden();
      await expect(page.getByRole('button',{name:`Guest OS ${width}`,exact:true})).toBeVisible();
      const actual=await rpc({oneofKind:'read',read:m.Read.create({target:{assetId:target.asset_id,incarnation:target.incarnation}})});
      expect(actual.detail.item.kind).toBe(m.AssetKind.VIRTUAL_MACHINE);
      expect(actual.detail.item.fields.find(v=>v.name==='name').value.text).toBe(`Known VM ${width}`);
    }
  }finally{await server.stop();await rm(dir,{recursive:true,force:true});}
});

test('native collector file imports into the actual disk-backed UI', async ({ page }) => {
  // First adopt a subset, then all actually collected components and selected
  // network sections. Private facts live only in the test-owned directory.
  test.setTimeout(120_000);
  const dir = await mkdtemp(join(tmpdir(), 'omloom-native-'));
  const server = await host(join(dir, 'database'));
  try {
    const filename = join(dir, 'host.omloom');
    await promisify(execFile)('target/debug/omloom-observe', ['--output', filename], { timeout: 100_000 });
    await page.setViewportSize({ width: 390, height: 844 });
    await page.goto(server.url);
    await page.getByRole('button', { name: 'More', exact: true }).click();
    await page.getByRole('button', { name: 'Import host report', exact: true }).click();
    const dialog = page.getByRole('dialog');
    await dialog.getByLabel('Host report file').setInputFiles(filename);
    await dialog.getByLabel('Source name', { exact: true }).fill('Native acceptance source');
    await expect(dialog.getByText('interfaces: verified', { exact: true })).toBeVisible();
    await dialog.getByLabel('New execution host type',{exact:true}).selectOption({label:'Physical device'});
    await dialog.getByRole('checkbox').nth(0).check();
    await dialog.getByRole('checkbox').nth(1).check();
    await dialog.getByRole('button', { name: 'Prepare host preview', exact: true }).click();
    await expect(dialog.getByRole('heading', { name: 'Review 2 selected resources', exact: true })).toBeVisible();
    await dialog.getByRole('button', { name: 'Accept selected observations', exact: true }).click();
    await expect(dialog).toBeHidden();
    const { m, rpc } = await workspaceClient(server.url);
    const response = await rpc({ oneofKind: 'query', query: m.Query.create({ kinds: [m.AssetKind.DEVICE], limit: 25 }) });
    expect(response.page.total).toBe(1n);
    const frame=m.Request.fromBinary(new Uint8Array(await readFile(filename)));
    const report=frame.command.hostImport.report;
    const open=page.getByRole('button',{name:'Import host report',exact:true});
    if(!await open.isVisible())await page.getByRole('button',{name:'More',exact:true}).click();
    await open.click();
    await dialog.getByLabel('Host report file').setInputFiles(filename);
    await dialog.getByLabel('Source name',{exact:true}).fill('Native acceptance source');
    for(let index=0;index<report.findings.length;index++)await dialog.getByRole('checkbox').nth(index).check();
    await dialog.getByLabel('Binds of selected services',{exact:true}).check();
    await dialog.getByLabel(/Local routing table/).check();
    await dialog.getByLabel(/Application firewall settings and rules/).check();
    await dialog.getByRole('button',{name:'Prepare host preview',exact:true}).click();
    await expect(dialog.getByRole('heading',{name:`Review ${report.findings.length} selected resources`,exact:true})).toBeVisible({timeout:15000});
    await dialog.getByRole('button',{name:'Accept selected observations',exact:true}).click();
    await expect(dialog).toBeHidden({timeout:15000});
    for(const kind of [m.AssetKind.INTERFACE,m.AssetKind.SERVICE,m.AssetKind.WORKLOAD,m.AssetKind.CONTAINER]) {
      const actual=await rpc({oneofKind:'query',query:m.Query.create({kinds:[kind],limit:25})});
      expect(actual.page.total).toBe(BigInt(report.findings.filter(v=>v.kind===kind).length));
    }
    const name=key=>report.findings.find(v=>v.key===key).fields.find(v=>v.name==='name').value.text;
    await page.getByRole('button',{name:name('hardware'),exact:true}).click();
    await page.getByRole('button',{name:'Use as scope',exact:true}).click();
    await page.getByRole('button',{name:name('installation'),exact:true}).click();
    await expect(page.getByRole('region',{name:'Resource details'}).getByRole('heading',{name:name('installation'),exact:true})).toBeVisible();
    const osURL=page.url();
    const network=page.getByRole('region',{name:'Local network observations'});
    const sockets=network.filter({hasText:'Native acceptance source / sockets'});
    if(report.binds.some(v=>v.owner))await expect(sockets.getByRole('row').nth(1)).toBeVisible();
    const routes=network.filter({hasText:'Native acceptance source / routes'});
    await routes.getByRole('button',{name:/^Routes \(/}).click();
    if(report.network.routes.length)await expect(routes.getByRole('row').nth(1)).toBeVisible();
    const firewall=network.filter({hasText:'Native acceptance source / application-fw'});
    await firewall.getByRole('button',{name:/^Firewall \(/}).click();
    await expect(firewall.getByRole('status')).toContainText('Effective per-port decision: unknown');
    for(const provider of report.findings.filter(v=>v.key==='netbird'||(v.key.startsWith('docker/')&&v.parent==='installation'))) {
      await page.goto(osURL);
      await expect(page.getByRole('region',{name:'Resource details'}).getByRole('heading',{name:name('installation'),exact:true})).toBeVisible();
      await page.getByRole('button',{name:'Use as scope',exact:true}).click();
      const providerName=name(provider.key);
      await page.getByLabel('Search inventory',{exact:true}).fill(providerName);
      await page.getByRole('button',{name:'Search',exact:true}).click({timeout:5000});
      const providerRow=page.getByRole('region',{name:'Inventory',exact:true}).getByRole('button',{name:providerName,exact:true});
      await expect(providerRow).toBeVisible();
      await providerRow.click();
      await expect(page.getByRole('region',{name:'Resource details'}).getByRole('heading',{name:providerName,exact:true})).toBeVisible();
      if(provider.key==='netbird') {
        await page.getByRole('button',{name:'Use as scope',exact:true}).click();
        await page.getByLabel('Search inventory',{exact:true}).fill('');
        await page.getByRole('button',{name:'Search',exact:true}).click();
        const interfaces=report.findings.filter(v=>v.parent===provider.key&&v.kind===m.AssetKind.INTERFACE);
        expect(interfaces.length).toBeGreaterThan(0);
        await page.getByRole('region',{name:'Inventory',exact:true}).getByRole('button',{name:name(interfaces[0].key),exact:true}).click();
        await expect(page.getByRole('region',{name:'Current interface addresses'})).toContainText(interfaces[0].addresses[0].address);
      }
    }
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  } finally { await server.stop(); await rm(dir, { recursive: true, force: true }); }
});

test('selecting another report cannot be overwritten by a pending file read', async ({ page }) => {
  // A slow removable/network-backed file can finish after a replacement file.
  // Its pending bytes cannot replace the user's newer explicit selection.
  const dir=await mkdtemp(join(tmpdir(),'omloom-report-read-'));const server=await host(dir);
  try {
    const {m}=await workspaceClient(server.url);
    const bytes=name=>Buffer.from(m.Request.toBinary(m.Request.create({command:{oneofKind:'hostImport',hostImport:m.HostImport.create({report:m.HostReport.create({version:1,platform:'macos',observedAt:100n,sourceBasis:'a'.repeat(64),bootBasis:'b'.repeat(64),findings:[{key:'hardware',kind:m.AssetKind.DEVICE,fields:[{name:'name',value:{oneofKind:'text',text:name}}]}]})})}})));
    await page.goto(server.url);await page.getByRole('button',{name:'Import host report',exact:true}).click();
    await page.evaluate(()=>{
      const original=File.prototype.arrayBuffer;
      const gate=new Promise(resolve=>{globalThis.releaseReportRead=resolve;});
      File.prototype.arrayBuffer=function(){
        const result=this.name==='earlier.omloom'?gate.then(()=>original.call(this)):original.call(this);
        if(this.name==='earlier.omloom')globalThis.pendingReportRead=result;
        return result;
      };
    });
    const dialog=page.getByRole('dialog');const file=dialog.getByLabel('Host report file');
    await file.setInputFiles({name:'earlier.omloom',mimeType:'application/octet-stream',buffer:bytes('Earlier file')});
    await file.setInputFiles({name:'selected.omloom',mimeType:'application/octet-stream',buffer:bytes('Selected file')});
    await expect(dialog.getByRole('checkbox',{name:'device: Selected file',exact:true})).toBeVisible();
    await page.evaluate(()=>{globalThis.releaseReportRead();return globalThis.pendingReportRead;});
    await expect(dialog.getByRole('checkbox',{name:'device: Selected file',exact:true})).toBeVisible();
    await expect(dialog.getByRole('checkbox',{name:'device: Earlier file',exact:true})).toHaveCount(0);
  }finally{await server.stop();await rm(dir,{recursive:true,force:true});}
});

test('optional runtime definitions and workload facts work on desktop and phone', async ({ page }) => {
  // Real forms and disk-backed calls preserve separate catalog, installation,
  // deployment and runtime lifetimes without inferring launch commands or owners.
  const dir = await mkdtemp(join(tmpdir(), 'omloom-software-')); const server = await host(dir);
  try {
    const { m, rpc } = await workspaceClient(server.url);
    for (const width of [1440, 390]) {
      await page.setViewportSize({ width, height: 900 }); await page.goto(`${server.url}/#/?view=4`);
      await expect(page.getByRole('region', { name: 'Resource details' })).toBeHidden();
      const product = await create(page, `Private product ${width}`, 'SOFTWARE_PRODUCT');
      for (const [suffix, version] of [['train A', 'Vendor 1.2 (LTS)'], ['train B', 'r2026 / build 7']]) {
        await page.getByRole('button', { name: 'Close resource details', exact: true }).click();
        await page.getByRole('button', { name: '+ Create resource', exact: true }).click();
        const dialog = page.getByRole('dialog');
        await dialog.getByLabel('Resource kind', { exact: true }).selectOption({ label: 'software release' });
        await dialog.getByLabel('Name', { exact: true }).fill(`${suffix} ${width}`);
        await dialog.getByLabel('Version change', { exact: true }).selectOption('set');
        await dialog.getByLabel('Version', { exact: true }).fill(version);
        await expect(dialog.getByLabel('Version interpretation', { exact: true })).toHaveCount(0);
        await dialog.getByRole('button', { name: 'Save resource', exact: true }).click();
        await expect(dialog).toBeHidden();
        await expect(page.getByRole('region', { name: 'Resource details' }).getByRole('heading', { name: `${suffix} ${width}`, exact: true })).toBeVisible();
        const hash = new URLSearchParams(new URL(page.url()).hash.split('?')[1]);
        const target = { assetId: hash.get('asset'), incarnation: hash.get('incarnation') };
        await rpc({ oneofKind: 'relation', relation: m.Relationship.create({ relation: { from: target, to: { assetId: product.asset_id, incarnation: product.incarnation }, kind: m.RelationKind.RELEASE_OF } }) });
      }
      await page.reload();
      await expect(page.getByRole('region', { name: 'Resource details' })).toContainText('r2026 / build 7');
      await expect(page.getByRole('region', { name: 'Release comparison' })).toHaveCount(0);
      const workload = await create(page, `Research workload ${width}`, 'WORKLOAD');
      await page.getByRole('button', { name: 'Edit resource', exact: true }).click();
      const editor = page.getByRole('dialog');
      await editor.getByLabel('Deployment kind change', { exact: true }).selectOption('set');
      await editor.getByLabel('Deployment kind', { exact: true }).selectOption('WORKLOAD_KIND_COMPOSE');
      await editor.getByLabel('Ordered Compose file references (one per line) change', { exact: true }).selectOption('set');
      await editor.getByLabel('Ordered Compose file references (one per line)', { exact: true }).fill('/project/compose.yml\n/project/local.yml');
      await editor.getByRole('button', { name: 'Save resource', exact: true }).click();
      await expect(editor).toBeHidden();
      await page.reload();
      await expect(page.getByRole('region', { name: 'Resource details' })).toContainText('/project/compose.yml');
      await expect(page.getByRole('region', { name: 'Ownership and responsibility' }).locator('dt').filter({ hasText: /^Claimed owner$/ }).locator('+ dd')).toHaveText('unknown');
      await expect(page.getByRole('button', { name: /start workload|stop workload/i })).toHaveCount(0);
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
      expect(workload.asset_id).toBeTruthy();
      await page.getByRole('button', { name: 'Close resource details', exact: true }).click();
      await expect(page.getByRole('region', { name: 'Resource details' })).toBeHidden();
      await page.getByRole('button', { name: 'Add filter', exact: true }).click();
      const filter = page.getByRole('dialog');
      await filter.getByLabel('Field', { exact: true }).selectOption('workload_kind');
      await filter.getByRole('combobox', { name: 'Equals', exact: true }).selectOption({ label: 'compose' });
      await filter.getByRole('button', { name: 'Apply filter', exact: true }).click();
      await expect(filter).toBeHidden();
      const inventory = page.getByRole('region', { name: 'Inventory', exact: true });
      await expect(inventory.getByRole('button', { name: `Research workload ${width}`, exact: true })).toBeVisible();
      await expect(inventory.getByRole('button', { name: `Private product ${width}`, exact: true })).toHaveCount(0);
    }
  } finally { await server.stop(); await rm(dir, { recursive: true, force: true }); }
});

test('runtime context creates workloads and their containers without flattening the inventory', async ({ page }) => {
  // A runtime lists deployments, then each deployment lists its owned instances.
  // Runtime identity links do not pull all instances into the enclosing list.
  const dir = await mkdtemp(join(tmpdir(), 'omloom-runtime-')); const server = await host(dir);
  try {
    const { m, rpc } = await workspaceClient(server.url);
    for (const width of [1440, 390]) {
      const runtime = (await rpc({ oneofKind: 'write', write: m.Write.create({ kind: m.AssetKind.SOFTWARE_INSTALLATION, changes: [{ name: 'name', value: { oneofKind: 'text', text: `Runtime ${width}` } }, { name: 'installation_kind', value: { oneofKind: 'text', text: 'INSTALLATION_KIND_CONTAINER_RUNTIME' } }] }) })).mutation.targets[0];
      await page.setViewportSize({ width, height: 900 });
      await page.goto(`${server.url}/#/?view=4&scope=${runtime.assetId}&scopeInc=${runtime.incarnation}`);
      await expect(page.getByRole('region', { name: 'Resource details' })).toBeHidden();
      await page.getByRole('button', { name: '+ Create resource', exact: true }).click();
      let editor = page.getByRole('dialog');
      await editor.getByLabel('Resource kind', { exact: true }).selectOption({ label: 'workload' });
      await expect(editor.getByRole('option', { name: 'device', exact: true })).toHaveCount(0);
      await expect(editor.getByRole('option', { name: 'port', exact: true })).toHaveCount(0);
      await editor.getByLabel('Name', { exact: true }).fill(`Deployment ${width}`);
      await editor.getByRole('button', { name: 'Save resource', exact: true }).click(); await expect(editor).toBeHidden();
      await expect(page.getByRole('region', { name: 'Resource details' }).getByRole('heading', { name: `Deployment ${width}`, exact: true })).toBeVisible();
      const deploymentHash = new URLSearchParams(new URL(page.url()).hash.split('?')[1]);
      const deployment = { assetId: deploymentHash.get('asset'), incarnation: deploymentHash.get('incarnation') };
      const current = await rpc({ oneofKind: 'read', read: m.Read.create({ target: deployment }) });
      await rpc({ oneofKind: 'responsibility', responsibility: m.Responsibility.create({ target: deployment, expected: current.detail.item.revision, action: m.ResponsibilityAction.APPLY,
        changes: [{ facet: m.ResponsibilityFacet.CLAIMED_OWNER, mode: m.AssignmentMode.EXPLICIT, party: { issuer: 'fixture.company', subject: `team-${width}`, name: `Team ${width}`, kind: m.PartyKind.TEAM } }] }) });
      await page.reload();
      await expect(page.getByRole('region', { name: 'Resource details' }).getByRole('heading', { name: `Deployment ${width}`, exact: true })).toBeVisible();
      await page.getByRole('button', { name: '+ Create resource', exact: true }).first().click();
      editor = page.getByRole('dialog');
      await editor.getByLabel('Resource kind', { exact: true }).selectOption({ label: 'container' });
      await editor.getByLabel('Name', { exact: true }).fill(`Instance ${width}`);
      await editor.getByLabel('Runtime-local container ID change', { exact: true }).selectOption('set');
      await editor.getByLabel('Runtime-local container ID', { exact: true }).fill(`container-${width}`);
      await editor.getByLabel('Runtime lifetime evidence change', { exact: true }).selectOption('set');
      await editor.getByLabel('Runtime lifetime evidence', { exact: true }).fill(`creation-${width}`);
      await editor.getByRole('button', { name: 'Save resource', exact: true }).click(); await expect(editor).toBeHidden();
      await expect(page.getByRole('region', { name: 'Resource details' })).toContainText(`container-${width}`);
      const inheritedOwner = page.getByRole('region', { name: 'Ownership and responsibility' }).locator('dt').filter({ hasText: /^Claimed owner$/ }).locator('+ dd');
      await expect(inheritedOwner).toContainText(`Team ${width}`);
      await expect(inheritedOwner).toContainText('inherited');
      const hash = new URLSearchParams(new URL(page.url()).hash.split('?')[1]);
      await rpc({ oneofKind: 'relation', relation: m.Relationship.create({ relation: { from: { assetId: hash.get('asset'), incarnation: hash.get('incarnation') }, to: runtime, kind: m.RelationKind.RUNS_ON } }) });
      await page.goto(`${server.url}/#/?view=4&scope=${runtime.assetId}&scopeInc=${runtime.incarnation}`);
      await expect(page.getByRole('region', { name: 'Resource details' })).toBeHidden();
      const inventory = page.getByRole('region', { name: 'Inventory', exact: true });
      await expect(inventory.getByRole('button', { name: `Deployment ${width}`, exact: true })).toBeVisible();
      await expect(inventory.getByRole('button', { name: `Instance ${width}`, exact: true })).toHaveCount(0);
      await page.getByRole('navigation', { name: 'Resource categories' }).getByRole('button', { name: 'Workloads', exact: true }).click();
      await inventory.getByRole('button', { name: `Show resources owned by Team ${width}`, exact: true }).click();
      await expect(inventory.getByRole('button', { name: `Deployment ${width}`, exact: true })).toBeVisible();
      expect(JSON.parse(new URLSearchParams(new URL(page.url()).hash.split('?')[1]).get('responsibility')).subject).toBe(`team-${width}`);
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    }
  } finally { await server.stop(); await rm(dir, { recursive: true, force: true }); }
});

test('responsibility editing, inherited components and typed import work on desktop and phone', async ({ page }) => {
  // A real component inherits its accepted source; an explicit stop survives parent edits and reload.
  const dir=await mkdtemp(join(tmpdir(),'omloom-responsibility-')); const server=await host(dir);
  try {
    const {m,rpc}=await workspaceClient(server.url);
    for(const width of [1440,390]) {
      await page.setViewportSize({width,height:900});await page.goto(server.url);
      const created=await create(page,`Server ${width}`);
      const parent={assetId:created.asset_id,incarnation:created.incarnation};
      const panel=page.getByRole('region',{name:'Ownership and responsibility'});
      await page.getByRole('button',{name:'Edit claimed owner',exact:true}).click();
      let dialog=page.getByRole('dialog');
      await dialog.getByLabel('Party namespace',{exact:true}).fill('company.example');
      await dialog.getByLabel('Party identity',{exact:true}).fill('devops');
      await dialog.getByLabel('Party label',{exact:true}).fill('DevOps');
      await dialog.getByRole('button',{name:'Review assignment',exact:true}).click();
      await expect(dialog.getByRole('heading',{name:'Review effective result'})).toBeVisible();
      await dialog.getByRole('button',{name:'Apply assignment',exact:true}).click();
      await expect(dialog).toBeHidden();await expect(panel.getByText('DevOps',{exact:true})).toBeVisible();
      const child=(await rpc({oneofKind:'write',write:m.Write.create({kind:m.AssetKind.INTERFACE,parent,changes:[{name:'name',value:{oneofKind:'text',text:`NIC ${width}`}}]})},parent)).mutation.targets[0];
      await page.goto(`${server.url}/#/?view=1&asset=${child.assetId}&incarnation=${child.incarnation}`);
      await expect(panel.getByText('DevOps',{exact:true})).toBeVisible();
      await page.getByRole('button',{name:'Edit claimed owner',exact:true}).click();
      dialog=page.getByRole('dialog');await dialog.getByLabel('Assignment',{exact:true}).selectOption(String(m.AssignmentMode.UNASSIGNED));
      await dialog.getByRole('button',{name:'Review assignment',exact:true}).click();await dialog.getByRole('button',{name:'Apply assignment',exact:true}).click();await expect(dialog).toBeHidden();
      const current=(await rpc({oneofKind:'read',read:m.Read.create({target:parent})},parent)).detail.item;
      await rpc({oneofKind:'responsibility',responsibility:m.Responsibility.create({target:parent,expected:current.revision,action:m.ResponsibilityAction.APPLY,changes:[m.ResponsibilityAssignment.create({facet:m.ResponsibilityFacet.CLAIMED_OWNER,mode:m.AssignmentMode.EXPLICIT,party:m.PartyRef.create({issuer:'company.example',subject:'another',kind:m.PartyKind.TEAM,name:'Another team'})})]})},parent);
      await page.reload();await expect(panel.getByText('unassigned',{exact:true})).toBeVisible();await expect(panel.getByText('Another team',{exact:true})).toHaveCount(0);
      const state=(await rpc({oneofKind:'responsibility',responsibility:m.Responsibility.create({target:child,action:m.ResponsibilityAction.READ})},child)).responsibility;
      const transfer=m.ResponsibilityTransfer.create({entries:[m.Responsibility.create({target:child,expected:state.item.revision,action:m.ResponsibilityAction.APPLY,changes:[m.ResponsibilityAssignment.create({facet:m.ResponsibilityFacet.CLAIMED_OWNER,mode:m.AssignmentMode.EXPLICIT,party:m.PartyRef.create({issuer:'company.example',subject:'developer-a',kind:m.PartyKind.TEAM,name:'Developer A'})})]})]});
      await page.getByRole('button',{name:'Import responsibility',exact:true}).click();dialog=page.getByRole('dialog');
      await dialog.getByLabel('Typed JSON import',{exact:true}).fill(JSON.stringify(m.ResponsibilityTransfer.toJson(transfer)));
      await dialog.getByRole('button',{name:'Review import',exact:true}).click();await expect(dialog.getByText('1 resources validated. No changes have been saved.',{exact:true})).toBeVisible();
      const bounds=await dialog.boundingBox();expect(bounds.width).toBeLessThanOrEqual(width);
      await dialog.getByRole('button',{name:'Apply import',exact:true}).click();await expect(dialog).toBeHidden();await expect(panel.getByText('Developer A',{exact:true})).toBeVisible();
      await page.goto(`${server.url}/#/?view=1`);
      await page.getByRole('button',{name:'Add filter',exact:true}).click();dialog=page.getByRole('dialog');
      await dialog.getByLabel('Filter by effective responsibility',{exact:true}).check();
      await dialog.getByLabel('Responsibility facet',{exact:true}).selectOption(String(m.ResponsibilityFacet.CLAIMED_OWNER));
      await dialog.getByLabel('Party namespace',{exact:true}).fill('company.example');await dialog.getByLabel('Party identity',{exact:true}).fill('developer-a');
      await dialog.getByRole('button',{name:'Apply filter',exact:true}).click();await expect(dialog).toBeHidden();
      await expect(page.getByRole('region',{name:'Inventory'}).getByText(`NIC ${width}`,{exact:true})).toBeVisible();
      await page.getByRole('button',{name:'Save view',exact:true}).click();dialog=page.getByRole('dialog');await dialog.getByLabel('Name',{exact:true}).fill(`Owned NIC ${width}`);
      await dialog.getByRole('button',{name:'Save personal view',exact:true}).click();await expect(dialog).toBeHidden();
      await page.getByRole('button',{name:'Remove responsibility filter',exact:true}).click();
      await page.getByRole('button',{name:`◦ Owned NIC ${width}`,exact:true}).click();
      await expect(page.getByRole('button',{name:'Remove responsibility filter',exact:true})).toBeVisible();
      expect(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth)).toBe(true);
    }
  }finally{await server.stop();await rm(dir,{recursive:true,force:true});}
});

test('personal registrar owner requests retain exact changes without claiming provider success',async({page})=>{
  // Metadata, bindings, approval and an owner assertion traverse the real API and durable store.
  const dir=await mkdtemp(join(tmpdir(),'omloom-owner-request-'));const server=await host(dir);
  try {
    const {m}=await workspaceClient(server.url);
    for(const width of [1440,390]) {
      await page.setViewportSize({width,height:900});await page.goto(server.url);
      await create(page,`Company domain ${width}`,'DOMAIN');
      await create(page,`Founder registrar ${width}`,'PROVIDER_ACCOUNT');
      await page.getByRole('button',{name:'Edit account metadata',exact:true}).click();let dialog=page.getByRole('dialog');
      await dialog.getByLabel('Provider namespace',{exact:true}).fill('registrar.example');await dialog.getByLabel('Provider account identity',{exact:true}).fill(`personal-${width}`);
      await dialog.getByLabel('Account kind',{exact:true}).selectOption(String(m.ExternalAccountKind.ADMINISTRATION));await dialog.getByLabel('Provider delegation',{exact:true}).selectOption(String(m.AccountDelegation.OWNER_ONLY));
      await dialog.getByRole('button',{name:'Save account metadata',exact:true}).click();await expect(dialog).toBeHidden();
      await page.getByRole('button',{name:'Bind selected resource',exact:true}).click();dialog=page.getByRole('dialog');
      await dialog.getByLabel('Find resource',{exact:true}).fill(`Company domain ${width}`);await dialog.getByRole('button',{name:'Find resources',exact:true}).click();
      await dialog.getByRole('button',{name:new RegExp(`Company domain ${width}`)}).click();
      await dialog.getByRole('button',{name:'Review binding',exact:true}).click();
      dialog=page.getByRole('dialog',{name:'Review account binding',exact:true});
      await expect(dialog.getByRole('region',{name:'Selected relationship context'})).toContainText(`Company domain ${width}`);
      await expect(dialog.getByRole('region',{name:'Selected relationship context'})).toContainText(`Founder registrar ${width}`);
      await dialog.getByRole('button',{name:'Save binding',exact:true}).click();await expect(dialog).toBeHidden();
      await page.getByRole('button',{name:'Edit account controller',exact:true}).click();dialog=page.getByRole('dialog');
      await dialog.getByLabel('Party kind',{exact:true}).selectOption(String(m.PartyKind.INDIVIDUAL));
      await dialog.getByLabel('Party namespace',{exact:true}).fill('people.example');await dialog.getByLabel('Party identity',{exact:true}).fill('founder');await dialog.getByLabel('Party label',{exact:true}).fill('Founder');
      await dialog.getByRole('button',{name:'Review assignment',exact:true}).click();await dialog.getByRole('button',{name:'Apply assignment',exact:true}).click();await expect(dialog).toBeHidden();
      await expect(page.getByRole('region',{name:'Ownership and responsibility'}).getByText('Founder',{exact:true})).toBeVisible();
      await page.getByRole('button',{name:'Share account view',exact:true}).click();dialog=page.getByRole('dialog');
      await dialog.getByLabel('Name',{exact:true}).fill(`Registrar reference ${width}`);await dialog.getByLabel('operations',{exact:true}).check();
      await dialog.getByRole('button',{name:'Preview publication',exact:true}).click();
      await expect(dialog.getByText("Data remains restricted by each reader's rights.",{exact:false})).toBeVisible();
      await dialog.getByRole('button',{name:'Publish reviewed definition',exact:true}).click();await expect(dialog).toBeHidden();
      await page.getByRole('button',{name:'Close resource details',exact:true}).click();
      await expect(page.getByRole('button',{name:`♧ Registrar reference ${width}`,exact:true})).toBeVisible();
      await page.getByRole('button',{name:`♧ Registrar reference ${width}`,exact:true}).click();
      await page.getByRole('button',{name:'Request owner action',exact:true}).click();dialog=page.getByRole('dialog');
      await dialog.getByLabel('Managed resource',{exact:true}).selectOption({label:`Company domain ${width}`});
      await dialog.getByLabel('Requested action',{exact:true}).fill('set-description');await dialog.getByLabel('Proposed value',{exact:true}).fill(`Company-managed domain ${width}`);
      await dialog.getByLabel('Request deadline',{exact:true}).fill(new Date(Date.now()+86400000).toISOString().slice(0,16));
      await dialog.getByRole('button',{name:'Save owner request',exact:true}).click();
      dialog=page.getByRole('dialog',{name:'Owner action request',exact:true});await expect(dialog.getByText(`Company-managed domain ${width}`,{exact:true})).toBeVisible();
      await dialog.getByRole('button',{name:'Approve requested change',exact:true}).click();await expect(dialog.getByText('Approved; waiting for the account owner.',{exact:true})).toBeVisible();
      await dialog.getByLabel('Owner-reported outcome',{exact:true}).selectOption(String(m.EffectOutcome.CONFIRMED));await dialog.getByRole('button',{name:'Record owner report',exact:true}).click();
      await expect(dialog.getByText('Owner-reported result. Independent provider verification is still required.',{exact:true})).toBeVisible();
      expect((await dialog.boundingBox()).width).toBeLessThanOrEqual(width);
      await dialog.getByRole('button',{name:'Close dialog',exact:true}).click();await page.reload();
      await page.getByRole('button',{name:`Company domain ${width} · Needs independent verification`,exact:true}).click();await expect(page.getByRole('dialog').getByText(`Company-managed domain ${width}`,{exact:true})).toBeVisible();
      await page.getByRole('dialog').getByRole('button',{name:'Close dialog',exact:true}).click();
    }
  }finally{await server.stop();await rm(dir,{recursive:true,force:true});}
});

test('partial account metadata cannot be cleared by a write-only replacement form',async({page})=>{
  // Hidden metadata is absent from replies, so a whole-record editor must not turn it into a clear.
  const dir=await mkdtemp(join(tmpdir(),'omloom-account-fields-'));let server=await host(dir);
  try {
    const {m,rpc}=await workspaceClient(server.url);
    const account=(await rpc({oneofKind:'write',write:m.Write.create({kind:m.AssetKind.PROVIDER_ACCOUNT,changes:[{name:'name',value:{oneofKind:'text',text:'Partially disclosed account'}}]})})).mutation.targets[0];
    const binding=account.assetId;
    await rpc({oneofKind:'externalAccount',externalAccount:m.ExternalAccount.create({target:account,expected:1n,metadata:{issuer:'provider.example',subject:'selected-account',kind:m.ExternalAccountKind.ADMINISTRATION,delegation:m.AccountDelegation.OWNER_ONLY,credentialBindingId:binding}})},account);
    await server.stop();
    const grants=join(dir,'grants.json');
    await writeFile(grants,JSON.stringify([{selection:{scope:'all'},read:{mode:'selected',fields:['name','issuer','subject','account_kind','delegation','login','sender','recovery_route','account.bindings','account.requests']},write:{mode:'all'},writable:true}]));
    server=await host(dir,'operator',grants);
    for(const width of [1440,390]) {
      await page.setViewportSize({width,height:900});
      await page.goto(`${server.url}/#/?view=0&asset=${account.assetId}&incarnation=${account.incarnation}`);
      const panel=page.getByRole('region',{name:'External account',exact:true});
      await expect(panel.getByText('provider.example',{exact:true})).toBeVisible();
      await expect(panel.getByRole('button',{name:'Edit account metadata',exact:true})).toHaveCount(0);
      await expect(panel).not.toContainText(binding);
    }
    await server.stop();server=await host(dir);
    const full=await workspaceClient(server.url);
    const saved=(await full.rpc({oneofKind:'externalAccount',externalAccount:full.m.ExternalAccount.create({target:account})},account)).externalAccount;
    expect(saved.metadata.credentialBindingId).toBe(binding);
  }finally{await server.stop();await rm(dir,{recursive:true,force:true});}
});

test('hidden responsibility contacts cannot be cleared through the replacement form', async ({ page }) => {
  // Contact write permission is independent of disclosure; absence in a reply is not an empty value.
  const dir=await mkdtemp(join(tmpdir(),'omloom-contact-fields-'));let server=await host(dir);
  try {
    const {m,rpc}=await workspaceClient(server.url);
    const target=(await rpc({oneofKind:'write',write:m.Write.create({kind:m.AssetKind.DEVICE,changes:[{name:'name',value:{oneofKind:'text',text:'Restricted contact'}}]})})).mutation.targets[0];
    await rpc({oneofKind:'responsibility',responsibility:m.Responsibility.create({target,expected:1n,action:m.ResponsibilityAction.APPLY,changes:[{facet:m.ResponsibilityFacet.CLAIMED_OWNER,mode:m.AssignmentMode.EXPLICIT,party:{issuer:'company.example',subject:'devops',kind:m.PartyKind.TEAM,name:'DevOps'},contact:'mailto:private@example.test'}]})},target);
    await server.stop();const grants=join(dir,'grants.json');
    await writeFile(grants,JSON.stringify([{selection:{scope:'all'},read:{mode:'selected',fields:['name','relations','claimed_owner']},write:{mode:'all'},writable:true}]));
    server=await host(dir,'operator',grants);
    for(const width of [1440,390]) {
      await page.setViewportSize({width,height:900});await page.goto(`${server.url}/#/?asset=${target.assetId}&incarnation=${target.incarnation}`);
      const panel=page.getByRole('region',{name:'Ownership and responsibility'});
      await expect(panel.getByText('DevOps',{exact:true})).toBeVisible();
      await expect(panel.getByRole('button',{name:'Edit claimed owner',exact:true})).toHaveCount(0);
      await expect(panel).not.toContainText('private@example.test');
    }
  }finally{await server.stop();await rm(dir,{recursive:true,force:true});}
});

test('leaving hypervisor through another resource kind discards its role', async ({ page }) => {
  // A hidden draft role must not make a subsequently selected ordinary installation a hypervisor.
  const dir = await mkdtemp(join(tmpdir(), 'omloom-kind-roundtrip-'));
  const server = await host(dir);
  try {
    const { m, rpc } = await workspaceClient(server.url);
    for (const width of [1440, 390]) {
      await page.setViewportSize({ width, height: 900 });
      await page.goto(`${server.url}/#/?view=0`);
      await page.getByRole('button', { name: '+ Create resource', exact: true }).click();
      const dialog = page.getByRole('dialog');
      const kind = dialog.getByLabel('Resource kind', { exact: true });
      await kind.selectOption({ label: 'Hypervisor (OS installation)' });
      await kind.selectOption({ label: 'device' });
      await kind.selectOption({ label: 'software installation' });
      await expect(dialog.getByLabel('Virtualization role', { exact: true })).toHaveValue('unknown');
      await dialog.getByLabel('Name', { exact: true }).fill(`Ordinary installation ${width}`);
      await dialog.getByRole('button', { name: 'Save resource', exact: true }).click();
      await expect(dialog).toBeHidden();
      await expect(page.getByRole('region', { name: 'Resource details' }).getByRole('heading', { name: `Ordinary installation ${width}`, exact: true })).toBeVisible();
      const params = new URLSearchParams(new URL(page.url()).hash.split('?')[1]);
      const target = { assetId: params.get('asset'), incarnation: params.get('incarnation') };
      const saved = await rpc({ oneofKind: 'read', read: m.Read.create({ target }) }, target);
      expect(saved.detail.item.fields.some(f => f.name === 'virtualization_role')).toBe(false);
    }
  } finally { await server.stop(); await rm(dir, { recursive: true, force: true }); }
});

test('a VLAN tag requires an initial IP before a creation request is sent', async ({ page }) => {
  // Invalid local address rows must be explained before mutation, with the draft available for correction.
  const dir = await mkdtemp(join(tmpdir(), 'omloom-vlan-only-'));
  const server = await host(dir);
  try {
    const { m, rpc } = await workspaceClient(server.url);
    let mutations = 0;
    page.on('request', request => {
      if (!request.url().includes('/omloom.workspace.v1.Workspace/')) return;
      const body = request.postDataBuffer();
      if (!body) return;
      const command = m.Request.fromBinary(body.subarray(5)).command;
      if (command.oneofKind === 'create' || command.oneofKind === 'write') mutations++;
    });
    for (const width of [1440, 390]) {
      await page.setViewportSize({ width, height: 900 });
      await page.goto(`${server.url}/#/?view=2`);
      await page.getByRole('button', { name: '+ Create resource', exact: true }).click();
      const dialog = page.getByRole('dialog');
      await dialog.getByLabel('Name', { exact: true }).fill(`Address draft ${width}`);
      await dialog.getByLabel('VLAN tag 1', { exact: true }).fill('42');
      const before = (await rpc({ oneofKind: 'query', query: m.Query.create({ limit: 1 }) })).page.revision;
      const sent = mutations;
      await dialog.getByRole('button', { name: 'Save resource', exact: true }).click();
      await expect(dialog.getByRole('alert')).toHaveText('Error: Enter an IP address when specifying a VLAN tag');
      expect(mutations).toBe(sent);
      expect((await rpc({ oneofKind: 'query', query: m.Query.create({ limit: 1 }) })).page.revision).toBe(before);
      await expect(dialog.getByLabel('VLAN tag 1', { exact: true })).toHaveValue('42');
      await dialog.getByLabel('IP address 1', { exact: true }).fill('192.0.2.10');
      await dialog.getByRole('button', { name: 'Save resource', exact: true }).click();
      await expect(dialog).toBeHidden();
      expect(mutations).toBe(sent + 1);
      await expect(page.getByRole('region', { name: 'Resource details' }).getByRole('heading', { name: `Address draft ${width}`, exact: true })).toBeVisible();
    }
  } finally { await server.stop(); await rm(dir, { recursive: true, force: true }); }
});

test('relationship review excludes ineligible hypervisors but permits removing their placements', async ({ page }) => {
  // New placement qualification must not strand an existing placement when the host role changes.
  const dir = await mkdtemp(join(tmpdir(), 'omloom-role-connections-'));
  const server = await host(dir);
  try {
    const { m, rpc } = await workspaceClient(server.url);
    for (const width of [1440, 390]) {
      const make = async (kind, name) => (await rpc({ oneofKind: 'write', write: m.Write.create({ kind, changes: [{ name: 'name', value: { oneofKind: 'text', text: name } }] }) })).mutation.targets[0];
      const host = await make(m.AssetKind.SOFTWARE_INSTALLATION, `OS ${width}`);
      const guest = await make(m.AssetKind.VIRTUAL_MACHINE, `Guest ${width}`);
      await page.setViewportSize({ width, height: 900 });
      const address = `${server.url}/#/?view=3&asset=${guest.assetId}&incarnation=${guest.incarnation}`;
      await page.goto(address);
      await page.getByRole('button', { name: 'Connections in broader scope', exact: true }).click();
      const dialog = page.getByRole('dialog');
      await dialog.getByRole('button', { name: 'Choose connection endpoints', exact: true }).click();
      await dialog.getByLabel('Other endpoint', { exact: true }).selectOption(`${host.assetId}/${host.incarnation}`);
      await dialog.getByRole('button', { name: 'Review connection', exact: true }).click();
      await expect(dialog.getByLabel('Relationship', { exact: true }).locator('option')).toHaveText(['Choose the relationship meaning', 'contains']);
      await dialog.getByRole('button', { name: 'Return to resource', exact: true }).click();
      await rpc({ oneofKind: 'write', write: m.Write.create({ target: host, kind: m.AssetKind.SOFTWARE_INSTALLATION, expected: 1n, changes: [{ name: 'virtualization_role', value: { oneofKind: 'text', text: 'VIRTUALIZATION_ROLE_HYPERVISOR' } }] }) });
      await rpc({ oneofKind: 'relation', relation: m.Relationship.create({ relation: { from: guest, to: host, kind: m.RelationKind.RUNS_ON } }) });
      await rpc({ oneofKind: 'write', write: m.Write.create({ target: host, kind: m.AssetKind.SOFTWARE_INSTALLATION, expected: 2n, changes: [{ name: 'virtualization_role', value: { oneofKind: 'text', text: 'VIRTUALIZATION_ROLE_NOT_HYPERVISOR' } }] }) });
      await page.reload();
      await page.getByRole('button', { name: 'Connections in broader scope', exact: true }).click();
      await dialog.getByRole('button', { name: 'Review existing connections', exact: true }).click();
      await dialog.getByRole('button', { name: 'Review removal', exact: true }).click();
      await expect(dialog.getByRole('button', { name: 'Remove reviewed relationship', exact: true })).toBeEnabled();
      await dialog.getByRole('button', { name: 'Remove reviewed relationship', exact: true }).click();
      await expect(dialog).toBeHidden();
      const remaining = await rpc({ oneofKind: 'read', read: m.Read.create({ target: guest }) });
      expect(remaining.detail.relations).toHaveLength(0);
    }
  } finally { await server.stop(); await rm(dir, { recursive: true, force: true }); }
});

test('a VM cannot run on its own installation through the relationship editor', async ({ page }) => {
  // Rejecting the cycle must leave the containment edge and discoverable root intact.
  const dir = await mkdtemp(join(tmpdir(), 'omloom-cycle-review-'));
  const server = await host(dir);
  try {
    const { m, rpc } = await workspaceClient(server.url);
    for (const width of [1440, 390]) {
      const guest = (await rpc({ oneofKind: 'write', write: m.Write.create({ kind: m.AssetKind.VIRTUAL_MACHINE, changes: [{ name: 'name', value: { oneofKind: 'text', text: `Cycle guest ${width}` } }] }) })).mutation.targets[0];
      const host = (await rpc({ oneofKind: 'write', write: m.Write.create({ kind: m.AssetKind.SOFTWARE_INSTALLATION, parent: guest, changes: [{ name: 'name', value: { oneofKind: 'text', text: `Nested hypervisor ${width}` } }, { name: 'virtualization_role', value: { oneofKind: 'text', text: 'VIRTUALIZATION_ROLE_HYPERVISOR' } }] }) }, guest)).mutation.targets[0];
      await page.setViewportSize({ width, height: 900 });
      await page.goto(`${server.url}/#/?view=3&asset=${guest.assetId}&incarnation=${guest.incarnation}`);
      await page.getByRole('button', { name: 'Connections in broader scope', exact: true }).click();
      const dialog = page.getByRole('dialog');
      await dialog.getByRole('button', { name: 'Choose connection endpoints', exact: true }).click();
      await dialog.getByLabel('Other endpoint', { exact: true }).selectOption(`${host.assetId}/${host.incarnation}`);
      await dialog.getByRole('button', { name: 'Review connection', exact: true }).click();
      await dialog.getByLabel('Relationship', { exact: true }).selectOption({ label: 'runs on' });
      const before = (await rpc({ oneofKind: 'query', query: m.Query.create({ limit: 1 }) })).page.revision;
      await dialog.getByRole('button', { name: 'Add relationship', exact: true }).click();
      await expect(dialog.getByRole('alert')).toContainText('INVALID_ARGUMENT');
      expect((await rpc({ oneofKind: 'query', query: m.Query.create({ limit: 1 }) })).page.revision).toBe(before);
      const detail = (await rpc({ oneofKind: 'read', read: m.Read.create({ target: guest }) })).detail;
      expect(detail.relations.map(r => r.kind)).toEqual([m.RelationKind.CONTAINS]);
      const roots = (await rpc({ oneofKind: 'query', query: m.Query.create({ view: m.View.VIRTUALIZATION, levelOnly: true, limit: 100 }) })).page;
      expect(roots.items.some(i => i.target.assetId === guest.assetId)).toBe(true);
      await dialog.getByRole('button', { name: 'Return to resource', exact: true }).click();
      await expect(dialog).toBeHidden();
    }
  } finally { await server.stop(); await rm(dir, { recursive: true, force: true }); }
});

test('placement defaults to unknown and description is editable immediately', async ({ page }) => {
  // Optional placement must not block creation or force a separate field-enable action.
  const dir = await mkdtemp(join(tmpdir(), 'omloom-create-inputs-'));
  const server = await host(dir);
  try {
    for (const width of [1440, 390]) {
      await page.setViewportSize({ width, height: 900 });
      await page.goto(`${server.url}/#/?view=3`);
      await expect(page.getByRole('region', { name: 'Resource details' })).toBeHidden();
      await page.getByRole('button', { name: '+ Create resource', exact: true }).click();
      const dialog = page.getByRole('dialog');
      await dialog.getByLabel('Resource kind', { exact: true }).selectOption({ label: 'Hypervisor (OS installation)' });
      await expect(dialog.getByRole('combobox', { name: 'Find server', exact: true })).toHaveAttribute('placeholder', 'Unknown');
      await expect(dialog.getByLabel('Description', { exact: true })).toBeEnabled();
      await dialog.getByLabel('Description', { exact: true }).fill('Direct description input');
      await dialog.getByLabel('Name', { exact: true }).fill(`Unknown placement ${width}`);
      await dialog.getByRole('button', { name: 'Save resource', exact: true }).click();
      await expect(dialog).toBeHidden();
      const detail = page.getByRole('region', { name: 'Resource details' });
      await expect(detail.getByRole('heading', { name: `Unknown placement ${width}`, exact: true })).toBeVisible();
      await expect(detail).toContainText('Direct description input');
    }
  } finally { await server.stop(); await rm(dir, { recursive: true, force: true }); }
});

test('observation previews remain read only and reviewed interface addresses save atomically', async ({ page }) => {
  // The protocol fixture exercises the real observer, draft review and disk store together.
  const provider = createServer((request, response) => {
    const responses = {
      '/api2/json/version': { version: '9.2.21' },
      '/api2/json/nodes': [{ node: 'Observed node' }],
      '/api2/json/nodes/Observed%20node/status': { cpuinfo: { model: 'Test CPU' }, memory: { total: 17179869184 } },
      '/api2/json/nodes/Observed%20node/network': [{ iface: 'vmbr0', cidr: '192.0.2.10/24', cidr6: '2001:db8::10/64', 'vlan-id': 42 }],
    };
    response.setHeader('Content-Type', 'application/json');
    response.end(JSON.stringify({ data: responses[request.url] }));
  });
  await new Promise(resolve => provider.listen(0, '127.0.0.1', resolve));
  const dir = await mkdtemp(join(tmpdir(), 'omloom-observe-review-'));
  const server = await host(dir);
  try {
    const { m, rpc } = await workspaceClient(server.url);
    for (const width of [1440, 390]) {
      await page.setViewportSize({ width, height: 900 });
      await page.goto(`${server.url}/#/?view=3`);
      await expect(page.getByRole('region', { name: 'Resource details' })).toBeHidden();
      const before = (await rpc({ oneofKind: 'query', query: m.Query.create({ limit: 1 }) })).page.revision;
      await page.getByRole('button', { name: '+ Create resource', exact: true }).click();
      const dialog = page.getByRole('dialog');
      await dialog.getByLabel('Resource kind', { exact: true }).selectOption({ label: 'Hypervisor (OS installation)' });
      await dialog.getByLabel('Name', { exact: true }).fill(`Reviewed node ${width}`);
      await dialog.getByLabel('API endpoint', { exact: true }).fill(`http://127.0.0.1:${provider.address().port}`);
      await dialog.getByText('Optional one-time credentials', { exact: true }).click();
      await dialog.getByLabel('Authentication', { exact: true }).selectOption('token');
      await dialog.getByLabel('Token ID', { exact: true }).fill('test@pve!reader');
      await dialog.getByLabel('Token secret', { exact: true }).fill('test-secret');
      await dialog.getByRole('button', { name: 'Observe', exact: true }).click();
      await expect(dialog.getByRole('heading', { name: 'Review observed values', exact: true })).toBeVisible();
      await expect(dialog.getByLabel('Token secret', { exact: true })).toHaveValue('');
      await expect(dialog.getByLabel('Token ID', { exact: true })).toHaveValue('');
      expect((await rpc({ oneofKind: 'query', query: m.Query.create({ limit: 1 }) })).page.revision).toEqual(before);
      await dialog.getByRole('button', { name: 'Use selected values', exact: true }).click();
      await expect(dialog.getByLabel('Name', { exact: true })).toHaveValue(`Reviewed node ${width}`);
      await expect(dialog.getByLabel('IP address 1', { exact: true })).toHaveValue('192.0.2.10/24');
      await dialog.getByLabel('IP address 1', { exact: true }).fill('192.0.2.10');
      await dialog.getByRole('button', { name: 'Save resource', exact: true }).click();
      await expect(dialog).toBeHidden();
      await expect(page.getByRole('region', { name: 'Resource details' }).getByRole('heading', { name: `Reviewed node ${width}`, exact: true })).toBeVisible();
      const params = new URLSearchParams(new URL(page.url()).hash.split('?')[1]);
      const parent = { assetId: params.get('asset'), incarnation: params.get('incarnation') };
      const local = (await rpc({ oneofKind: 'read', read: m.Read.create({ target: parent }) })).detail.related;
      expect(local).toHaveLength(1);
      expect(local[0].kind).toBe(m.AssetKind.INTERFACE);
      const interfaceDetail = (await rpc({ oneofKind: 'read', read: m.Read.create({ target: local[0].target }) })).detail;
      const addresses = interfaceDetail.related.filter(item => item.kind === m.AssetKind.ADDRESS_REFERENCE);
      expect(addresses).toHaveLength(2);
      const ipv4 = addresses.find(item => item.fields.some(field => field.name === 'name' && field.value.text === '192.0.2.10'));
      const fields = (await rpc({ oneofKind: 'read', read: m.Read.create({ target: ipv4.target }) })).detail.item.fields;
      expect(fields.some(field => field.name === 'prefix_length')).toBe(false);
      expect(fields.find(field => field.name === 'vlan_tag')?.value.integer).toBe(42n);
    }
  } finally { await server.stop(); await rm(dir, { recursive: true, force: true }); await new Promise(resolve => provider.close(resolve)); }
});

test('Enter in observation credentials never saves the resource on desktop and phone', async ({ page }) => {
  // Credential Enter must run the read-only observer and leave inventory unchanged until review/save.
  test.setTimeout(60_000);
  const provider = createServer((request, response) => {
    const responses = {
      '/api2/json/access/ticket': { ticket: 'test-ticket' },
      '/api2/json/version': { version: '9.2.21' },
      '/api2/json/nodes': [{ node: 'Credential node' }],
      '/api2/json/nodes/Credential%20node/status': {},
      '/api2/json/nodes/Credential%20node/network': [],
    };
    response.setHeader('Content-Type', 'application/json');
    response.end(JSON.stringify({ data: responses[request.url] }));
  });
  await new Promise(resolve => provider.listen(0, '127.0.0.1', resolve));
  const dir = await mkdtemp(join(tmpdir(), 'omloom-observe-enter-'));
  const server = await host(dir);
  try {
    const { m, rpc } = await workspaceClient(server.url);
    const before = (await rpc({ oneofKind: 'query', query: m.Query.create({ limit: 1 }) })).page.revision;
    for (const width of [1440, 390]) {
      await page.setViewportSize({ width, height: 900 });
      for (const [method, field] of [['login', 'Login including realm'], ['login', 'Password'], ['token', 'Token ID'], ['token', 'Token secret']]) {
        await test.step(`${field} at ${width}px`, async () => {
          await page.goto(`${server.url}/#/?view=${m.View.VIRTUALIZATION}`);
          await page.getByRole('button', { name: '+ Create resource', exact: true }).click();
          const dialog = page.getByRole('dialog');
          await dialog.getByLabel('Resource kind', { exact: true }).selectOption({ label: 'Hypervisor (OS installation)' });
          await dialog.getByLabel('Name', { exact: true }).fill('Unsaved credential draft');
          await dialog.getByLabel('API endpoint', { exact: true }).fill(`http://127.0.0.1:${provider.address().port}`);
          await dialog.getByText('Optional one-time credentials', { exact: true }).click();
          await dialog.getByLabel('Authentication', { exact: true }).selectOption(method);
          const labels = method === 'login' ? ['Login including realm', 'Password'] : ['Token ID', 'Token secret'];
          await dialog.getByLabel(labels[0], { exact: true }).fill(method === 'login' ? 'test@pve' : 'test@pve!reader');
          await dialog.getByLabel(labels[1], { exact: true }).fill('test-secret');
          await dialog.getByLabel(field, { exact: true }).press('Enter');
          await expect(dialog.getByRole('heading', { name: 'Review observed values', exact: true })).toBeVisible();
          await expect(dialog.getByLabel('Name', { exact: true })).toHaveValue('Unsaved credential draft');
          for (const label of labels) await expect(dialog.getByLabel(label, { exact: true })).toHaveValue('');
          expect((await rpc({ oneofKind: 'query', query: m.Query.create({ limit: 1 }) })).page.revision).toEqual(before);
          await dialog.getByRole('button', { name: 'Cancel', exact: true }).click();
          await expect(dialog).toBeHidden();
        });
      }
    }
  } finally { await server.stop(); await rm(dir, { recursive: true, force: true }); await new Promise(resolve => provider.close(resolve)); }
});

test('description edits persist for every resource kind on desktop and phone', async ({ page }) => {
  // Direct typing must leave Preserve automatically for every shared editor consumer.
  test.setTimeout(180_000);
  const dir = await mkdtemp(join(tmpdir(), 'omloom-description-kinds-'));
  const server = await host(dir);
  try {
    const { m, rpc } = await workspaceClient(server.url);
    const targets = new Map();
    const object = async kind => (await rpc({ oneofKind: 'write', write: m.Write.create({ kind, changes: [{ name: 'name', value: { oneofKind: 'text', text: `Description ${m.AssetKind[kind]}` } }] }) })).mutation.targets[0];
    for (const kind of Object.values(m.AssetKind).filter(value => typeof value === 'number' && value !== m.AssetKind.UNSPECIFIED)) {
      if ([m.AssetKind.IP_ADDRESS, m.AssetKind.PREFIX, m.AssetKind.VLAN, m.AssetKind.ADDRESS_GROUP, m.AssetKind.ADDRESS_REFERENCE].includes(kind)) continue;
      targets.set(kind, await object(kind));
    }
    for (const kind of [m.AssetKind.IP_ADDRESS, m.AssetKind.PREFIX, m.AssetKind.VLAN]) {
      const allocation = kind === m.AssetKind.VLAN
        ? { link: targets.get(m.AssetKind.L2_DOMAIN), vlanId: 42, sharing: m.SharedUse.EXCLUSIVE }
        : { cidr: kind === m.AssetKind.PREFIX ? '192.0.2.0/24' : '192.0.2.10/32', addressSpace: targets.get(m.AssetKind.ADDRESS_SPACE), sharing: m.SharedUse.EXCLUSIVE };
      targets.set(kind, (await rpc({ oneofKind: 'network', network: m.Network.create({ kind, name: `Description ${m.AssetKind[kind]}`, ...allocation }) })).mutation.targets[0]);
    }
    targets.set(m.AssetKind.ADDRESS_REFERENCE, (await rpc({ oneofKind: 'address', address: m.Address.create({ address: '198.51.100.10' }) })).mutation.targets[0]);
    const grouped = (await rpc({ oneofKind: 'network', network: m.Network.create({ kind: m.AssetKind.IP_ADDRESS, name: 'Grouped IP', cidr: '192.0.2.20/32', addressSpace: targets.get(m.AssetKind.ADDRESS_SPACE), sharing: m.SharedUse.ANYCAST, groupName: 'Description group' }) })).mutation.targets[0];
    const groupDetail = (await rpc({ oneofKind: 'read', read: m.Read.create({ target: grouped }) })).detail;
    targets.set(m.AssetKind.ADDRESS_GROUP, groupDetail.group.target);
    for (const width of [1440, 390]) {
      await page.setViewportSize({ width, height: 900 });
      for (const [kind, target] of targets) {
        await test.step(`${m.AssetKind[kind]} at ${width}px`, async () => {
          await page.goto(`${server.url}/#/?asset=${target.assetId}&incarnation=${target.incarnation}&view=0`);
          await expect(page.getByRole('region', { name: 'Resource details' })).toContainText(target.assetId);
          await page.getByRole('button', { name: 'Edit resource', exact: true }).click();
          const dialog = page.getByRole('dialog');
          if (kind === m.AssetKind.ADDRESS_REFERENCE) await dialog.getByRole('button', { name: 'More known details', exact: true }).click();
          await expect(dialog.getByLabel('Description change', { exact: true })).toHaveValue('preserve');
          await expect(dialog.getByLabel('Description', { exact: true })).toBeEnabled();
          const description = `Updated ${m.AssetKind[kind]} at ${width}`;
          await dialog.getByLabel('Description', { exact: true }).fill(description);
          await expect(dialog.getByLabel('Description change', { exact: true })).toHaveValue('set');
          await dialog.getByRole('button', { name: /Save (resource|known address)/, exact: true }).click();
          await expect(dialog).toBeHidden();
          await page.reload();
          await expect(page.getByRole('region', { name: 'Resource details' })).toContainText(description);
          const detail = (await rpc({ oneofKind: 'read', read: m.Read.create({ target }) })).detail;
          expect(detail.item.fields.find(field => field.name === 'description')?.value).toEqual({ oneofKind: 'text', text: description });
        });
      }
    }
  } finally { await server.stop(); await rm(dir, { recursive: true, force: true }); }
});

test('role changes clear a remounted server placement on desktop and phone', async ({ page }) => {
  // A picker showing Unknown must not submit a server selected before it was unmounted.
  const dir = await mkdtemp(join(tmpdir(), 'omloom-role-placement-'));
  const server = await host(dir);
  try {
    const { m, rpc } = await workspaceClient(server.url);
    for (const width of [1440, 390]) {
      await page.setViewportSize({ width, height: 900 });
      const hardware = (await rpc({ oneofKind: 'write', write: m.Write.create({ kind: m.AssetKind.DEVICE, changes: [{ name: 'name', value: { oneofKind: 'text', text: `Role server ${width}` } }] }) })).mutation.targets[0];
      await page.goto(`${server.url}/#/?view=${m.View.SERVICES}`);
      await page.getByRole('button', { name: '+ Create resource', exact: true }).click();
      const dialog = page.getByRole('dialog');
      await dialog.getByLabel('Resource kind', { exact: true }).selectOption({ label: 'software installation' });
      await dialog.getByLabel('Name', { exact: true }).fill(`Role installation ${width}`);
      const role = dialog.getByLabel('Virtualization role', { exact: true });
      await role.selectOption({ label: 'Installed and enabled' });
      await dialog.getByLabel('Find server', { exact: true }).fill(`Role server ${width}`);
      await dialog.getByRole('option', { name: `Use server: Role server ${width}`, exact: true }).click();
      await expect(dialog.getByRole('button', { name: 'Save resource', exact: true })).toBeEnabled();
      await role.selectOption('unknown');
      await expect(dialog.getByLabel('Find server', { exact: true })).toHaveCount(0);
      await role.selectOption({ label: 'Installed and enabled' });
      await expect(dialog.getByLabel('Find server', { exact: true })).toHaveValue('');
      await dialog.getByRole('button', { name: 'Save resource', exact: true }).click();
      await expect(dialog).toBeHidden();
      await expect(page.getByRole('region', { name: 'Resource details' }).getByRole('heading', { name: `Role installation ${width}`, exact: true })).toBeVisible();
      const params = new URLSearchParams(new URL(page.url()).hash.split('?')[1]);
      const target = { assetId: params.get('asset'), incarnation: params.get('incarnation') };
      const detail = (await rpc({ oneofKind: 'read', read: m.Read.create({ target }) })).detail;
      expect(detail.relations.some(r => r.from.assetId === hardware.assetId || r.to.assetId === hardware.assetId)).toBe(false);
      await page.reload();
      await expect(page.getByRole('region', { name: 'Resource details' }).getByRole('heading', { name: `Role installation ${width}`, exact: true })).toBeVisible();
    }
  } finally { await server.stop(); await rm(dir, { recursive: true, force: true }); }
});

test('hypervisor OS and VM placement work from estate and parent on desktop and phone', async ({ page }) => {
  // The form must create real installations and atomic placement, including unknown hardware.
  test.setTimeout(90_000);
  page.setDefaultTimeout(10_000);
  const dir = await mkdtemp(join(tmpdir(), 'omloom-hypervisor-placement-'));
  const server = await host(dir);
  try {
    const { m, rpc } = await workspaceClient(server.url);
    for (const width of [1440, 390]) {
      await page.setViewportSize({ width, height: 900 });
      const hardware = (await rpc({ oneofKind: 'write', write: m.Write.create({ kind: m.AssetKind.DEVICE, changes: [{ name: 'name', value: { oneofKind: 'text', text: `Server ${width}` } }] }) })).mutation.targets[0];
      await page.goto(`${server.url}/#/?view=${m.View.VIRTUALIZATION}`);
      await expect(page.getByRole('region', { name: 'Resource details' })).toBeHidden();
      await page.getByRole('button', { name: '+ Create resource', exact: true }).click();
      let dialog = page.getByRole('dialog');
      await dialog.getByLabel('Resource kind', { exact: true }).selectOption({ label: 'Hypervisor (OS installation)' });
      await dialog.getByLabel('Find server', { exact: true }).fill(`Server ${width}`);
      await dialog.getByRole('option', { name: `Use server: Server ${width}`, exact: true }).click();
      await dialog.getByLabel('Name', { exact: true }).fill(`Hypervisor ${width}`);
      await dialog.getByRole('button', { name: 'Save resource', exact: true }).click();
      await expect(dialog).toBeHidden();
      await expect(page.getByRole('region', { name: 'Resource details' }).getByRole('heading', { name: `Hypervisor ${width}`, exact: true })).toBeVisible();
      const params = new URLSearchParams(new URL(page.url()).hash.split('?')[1]);
      const hypervisor = { assetId: params.get('asset'), incarnation: params.get('incarnation') };
      const hostDetail = (await rpc({ oneofKind: 'read', read: m.Read.create({ target: hypervisor }) })).detail;
      expect(hostDetail.item.kind).toBe(m.AssetKind.SOFTWARE_INSTALLATION);
      expect(hostDetail.relations.some(r => r.from.assetId === hardware.assetId && r.to.assetId === hypervisor.assetId)).toBe(true);
      await page.getByRole('button', { name: '+ Create resource', exact: true }).click();
      dialog = page.getByRole('dialog');
      await dialog.getByLabel('Resource kind', { exact: true }).selectOption({ label: 'virtual machine' });
      await expect(dialog.getByText(`Hypervisor: Hypervisor ${width}`, { exact: true })).toBeVisible();
      await dialog.getByLabel('Name', { exact: true }).fill(`Guest ${width}`);
      await dialog.getByRole('button', { name: 'Save resource', exact: true }).click();
      await expect(dialog).toBeHidden();
      await expect(page.getByRole('region', { name: 'Resource details' }).getByRole('heading', { name: `Guest ${width}`, exact: true })).toBeVisible();
      await page.goto(`${server.url}/#/?view=${m.View.VIRTUALIZATION}`);
      await expect(page.getByRole('region', { name: 'Resource details' })).toBeHidden();
      await page.getByRole('button', { name: '+ Create resource', exact: true }).click();
      dialog = page.getByRole('dialog');
      await dialog.getByLabel('Find hypervisor', { exact: true }).fill(`Hypervisor ${width}`);
      await dialog.getByRole('option', { name: `Use hypervisor: Hypervisor ${width}`, exact: true }).click();
      await dialog.getByLabel('Name', { exact: true }).fill(`Estate guest ${width}`);
      await dialog.getByRole('button', { name: 'Save resource', exact: true }).click();
      await expect(dialog).toBeHidden();
      await expect(page.getByRole('region', { name: 'Resource details' }).getByRole('heading', { name: `Estate guest ${width}`, exact: true })).toBeVisible();
      await page.goto(`${server.url}/#/?view=${m.View.VIRTUALIZATION}`);
      await expect(page.getByRole('region', { name: 'Resource details' })).toBeHidden();
      await page.getByRole('button', { name: '+ Create resource', exact: true }).click();
      dialog = page.getByRole('dialog');
      await dialog.getByLabel('Resource kind', { exact: true }).selectOption({ label: 'Hypervisor (OS installation)' });
      await dialog.getByRole('button', { name: 'Use Proxmox VE defaults', exact: true }).click();
      await expect(dialog.getByLabel('Virtualization role', { exact: true }).locator('option:checked')).toHaveText('Installed and enabled');
      await dialog.getByLabel('Name', { exact: true }).fill(`Unplaced hypervisor ${width}`);
      await dialog.getByRole('button', { name: 'Save resource', exact: true }).click();
      await expect(dialog).toBeHidden();
      await expect(page.getByRole('region', { name: 'Resource details' }).getByRole('heading', { name: `Unplaced hypervisor ${width}`, exact: true })).toBeVisible();
      await page.reload();
      await expect(page.getByRole('region', { name: 'Resource details' }).getByRole('heading', { name: `Unplaced hypervisor ${width}`, exact: true })).toBeVisible();
      await page.goto(`${server.url}/#/?asset=${hardware.assetId}&incarnation=${hardware.incarnation}&view=${m.View.HARDWARE}`);
      await expect(page.getByRole('region', { name: 'Resource details' }).getByRole('heading', { name: `Server ${width}`, exact: true })).toBeVisible();
      await page.getByRole('button', { name: '+ Create resource', exact: true }).click();
      dialog = page.getByRole('dialog');
      await dialog.getByLabel('Resource kind', { exact: true }).selectOption({ label: 'Hypervisor (OS installation)' });
      await expect(dialog.getByText(`Server: Server ${width}`, { exact: true })).toBeVisible();
      await expect(dialog.getByRole('combobox', { name: 'Find server', exact: true })).toHaveCount(0);
      await dialog.getByLabel('Name', { exact: true }).fill(`Second OS ${width}`);
      await dialog.getByRole('button', { name: 'Save resource', exact: true }).click();
      await expect(dialog).toBeHidden();
      await expect(page.getByRole('region', { name: 'Resource details' }).getByRole('heading', { name: `Second OS ${width}`, exact: true })).toBeVisible();
      await page.goto(`${server.url}/#/?asset=${hardware.assetId}&incarnation=${hardware.incarnation}&view=${m.View.VIRTUALIZATION}`);
      await expect(page.getByRole('region', { name: 'Resource details' }).getByRole('heading', { name: `Server ${width}`, exact: true })).toBeVisible();
      await page.getByRole('button', { name: '+ Create resource', exact: true }).click();
      dialog = page.getByRole('dialog');
      await dialog.getByLabel('Resource kind', { exact: true }).selectOption({ label: 'virtual machine' });
      await dialog.getByLabel('Find hypervisor', { exact: true }).click();
      await dialog.getByRole('option', { name: `Use hypervisor: Second OS ${width}`, exact: true }).click();
      await dialog.getByLabel('Name', { exact: true }).fill(`Scoped guest ${width}`);
      await dialog.getByRole('button', { name: 'Save resource', exact: true }).click();
      await expect(dialog).toBeHidden();
      await expect(page.getByRole('region', { name: 'Resource details' }).getByRole('heading', { name: `Scoped guest ${width}`, exact: true })).toBeVisible();
      await expect(page.getByRole('alert')).toHaveCount(0);
    }
  } finally { await server.stop(); await rm(dir, { recursive: true, force: true }); }
});

test('phone placement pages are bounded and preserve the resource draft', async ({ page }, info) => {
  // A host beyond page one must be selectable without loading the entire estate.
  const dir = await mkdtemp(join(tmpdir(), 'omloom-placement-pages-'));
  const server = await host(dir);
  try {
    const { m, rpc } = await workspaceClient(server.url);
    for (let i = 0; i < 26; i++) await rpc({ oneofKind: 'write', write: m.Write.create({ kind: m.AssetKind.DEVICE, changes: [{ name: 'name', value: { oneofKind: 'text', text: `Node ${String(i).padStart(2, '0')}` } }] }) });
    const queries = [];
    page.on('request', request => {
      if (!request.url().endsWith('/omloom.workspace.v1.Workspace/Call')) return;
      const body = request.postDataBuffer();
      if (!body) return;
      const command = m.Request.fromBinary(body.subarray(5)).command;
      if (command.oneofKind === 'query') queries.push(command.query);
    });
    await page.setViewportSize({ width: 390, height: 900 });
    await page.goto(`${server.url}/#/?view=${m.View.VIRTUALIZATION}`);
    await page.getByRole('button', { name: '+ Create resource', exact: true }).click();
    const dialog = page.getByRole('dialog');
    await dialog.getByLabel('Resource kind', { exact: true }).selectOption({ label: 'Hypervisor (OS installation)' });
    await dialog.getByLabel('Name', { exact: true }).fill('Draft survives placement');
    await dialog.getByLabel('Find server', { exact: true }).click();
    await expect(dialog.getByRole('option', { name: /^Use server:/ })).toHaveCount(25);
    await dialog.getByRole('button', { name: 'Next placement choices', exact: true }).click();
    await dialog.getByRole('option', { name: 'Use server: Node 25', exact: true }).click();
    await expect(dialog.getByLabel('Name', { exact: true })).toHaveValue('Draft survives placement');
    expect(queries.filter(q => q.view === m.View.HARDWARE).every(q => q.limit === 25)).toBe(true);
    expect(queries.some(q => q.view === m.View.HARDWARE && q.cursor)).toBe(true);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    await expect(dialog.getByRole('button', { name: 'Save resource', exact: true })).toBeEnabled();
    await page.screenshot({ path: info.outputPath('hypervisor-placement-phone.png') });
    await dialog.getByRole('button', { name: 'Save resource', exact: true }).click();
    await expect(dialog).toBeHidden();
    await expect(page.getByRole('region', { name: 'Resource details' }).getByRole('heading', { name: 'Draft survives placement', exact: true })).toBeVisible();
  } finally { await server.stop(); await rm(dir, { recursive: true, force: true }); }
});

test('Unplaced combines with text and scoped address search in the browser', async ({ page }) => {
  // Search and namespace bindings must survive toggling the physical-placement filter.
  const dir = await mkdtemp(join(tmpdir(), 'omloom-unplaced-search-'));
  const server = await host(dir);
  try {
    const { m, rpc } = await workspaceClient(server.url);
    await rpc({ oneofKind: 'write', write: m.Write.create({ kind: m.AssetKind.DEVICE, changes: [{ name: 'name', value: { oneofKind: 'text', text: 'needle' } }] }) });
    const space = (await rpc({ oneofKind: 'write', write: m.Write.create({ kind: m.AssetKind.ADDRESS_SPACE, changes: [{ name: 'name', value: { oneofKind: 'text', text: 'space' } }] }) })).mutation.targets[0];
    await rpc({ oneofKind: 'network', network: m.Network.create({ kind: m.AssetKind.IP_ADDRESS, name: 'Address', cidr: '192.0.2.5/32', addressSpace: space, sharing: m.SharedUse.EXCLUSIVE }) });
    for (const width of [1440, 390]) {
      await page.setViewportSize({ width, height: 900 });
      await page.goto(`${server.url}/#/?view=${m.View.HARDWARE}`);
      await page.getByRole('checkbox', { name: 'Unplaced', exact: true }).check();
      await page.getByRole('textbox', { name: 'Search inventory', exact: true }).fill('needle');
      await page.getByRole('button', { name: 'Search', exact: true }).click();
      await expect(page.getByRole('button', { name: 'needle', exact: true })).toBeVisible();
      await page.goto(`${server.url}/#/?view=${m.View.NETWORK}&unplaced=1&scope=${space.assetId}&scopeInc=${space.incarnation}`);
      await page.getByRole('textbox', { name: 'Search inventory', exact: true }).fill('192.0.2.5');
      await page.getByRole('button', { name: 'Search', exact: true }).click();
      await expect(page.getByRole('button', { name: 'Address', exact: true })).toBeVisible();
      await expect(page.getByRole('alert')).toHaveCount(0);
    }
  } finally { await server.stop(); await rm(dir, { recursive: true, force: true }); }
});

test('VLAN allocated inside an L2 domain remains visible on desktop and phone', async ({ page }) => {
  // The form, namespace query and detail navigation must use the same L2 containment.
  const dir = await mkdtemp(join(tmpdir(), 'omloom-l2-vlan-'));
  const server = await host(dir);
  try {
    const { m, rpc } = await workspaceClient(server.url);
    for (const width of [1440, 390]) {
      const parent = (await rpc({ oneofKind: 'write', write: m.Write.create({ kind: m.AssetKind.L2_DOMAIN, changes: [{ name: 'name', value: { oneofKind: 'text', text: `LAN ${width}` } }] }) })).mutation.targets[0];
      await page.setViewportSize({ width, height: 900 });
      await page.goto(`${server.url}/#/?asset=${parent.assetId}&incarnation=${parent.incarnation}&view=${m.View.NETWORK}`);
      await page.getByRole('button', { name: 'Allocate IP / prefix', exact: true }).click();
      const dialog = page.getByRole('dialog');
      await dialog.getByLabel('Name', { exact: true }).fill(`VLAN ${width}`);
      await dialog.getByLabel('VLAN ID', { exact: true }).fill('10');
      await dialog.getByRole('button', { name: 'Allocate', exact: true }).click();
      await expect(dialog).toBeHidden();
      await page.getByRole('button', { name: `VLAN ${width}`, exact: true }).click();
      await expect(page.getByRole('region', { name: 'Resource details' }).getByRole('heading', { name: `VLAN ${width}`, exact: true })).toBeVisible();
      await page.getByRole('button', { name: width === 390 ? 'Back to parent' : 'Enclosing selection', exact: true }).click();
      await expect(page.getByRole('region', { name: 'Resource details' }).getByRole('heading', { name: `LAN ${width}`, exact: true })).toBeVisible();
      await expect(page.getByRole('button', { name: `VLAN ${width}`, exact: true })).toBeVisible();
      await expect(page.getByRole('alert')).toHaveCount(0);
    }
  } finally { await server.stop(); await rm(dir, { recursive: true, force: true }); }
});

test('large network evidence pages cross the real transport on desktop and phone', async ({ page }) => {
  // Accepted partial batches must remain accessible without truncating facts or their evidence dates.
  test.setTimeout(90_000);
  const dir = await mkdtemp(join(tmpdir(), 'omloom-evidence-pages-'));
  const server = await host(dir);
  try {
    const { m, rpc } = await workspaceClient(server.url);
    const target = (await rpc({ oneofKind: 'write', write: m.Write.create({ kind: m.AssetKind.SOFTWARE_INSTALLATION, changes: [{ name: 'name', value: { oneofKind: 'text', text: 'Observed OS' } }] }) })).mutation.targets[0];
    for (let base = 0; base < 2048; base += 128) await rpc({ oneofKind: 'hostNetwork', hostNetwork: m.HostNetwork.create({ installation: target, source: 'collector', basis: 'b'.repeat(128), observedAt: BigInt(100 + base), socketCoverage: 'not collected', routeCoverage: 'not collected', applicationCoverage: 'verified', packetFilterCoverage: 'not collected', rules: Array.from({ length: 128 }, (_, i) => ({ subject: String(base + i).padStart(4,'0') + 'x'.repeat(96), action: 'allow incoming' })) }) }, target);
    const first = (await rpc({ oneofKind: 'read', read: m.Read.create({ target }) }, target)).detail;
    expect(first.networkNext).toBeDefined();
    const next = (await rpc({ oneofKind: 'read', read: m.Read.create({ target, networkCursor: first.networkNext }) }, target)).detail;
    for (const detail of [first, next]) expect(m.Reply.toBinary(m.Reply.create({ result: { oneofKind: 'detail', detail } })).length).toBeLessThanOrEqual(512 * 1024);
    expect(next.item).toBeUndefined();
    const facts = [...first.hostNetwork, ...next.hostNetwork].flatMap(source => source.observations);
    expect(facts).toHaveLength(2048);
    expect(new Set(facts.map(o => o.value.rule.subject)).size).toBe(2048);
    for (const width of [1440, 390]) {
      await page.setViewportSize({ width, height: 900 });
      await page.goto(server.url);
      await expect(page.getByRole('button', { name: 'Observed OS', exact: true })).toBeVisible();
      await page.goto(`${server.url}/#/?asset=${target.assetId}&incarnation=${target.incarnation}`);
      await page.getByRole('button', { name: /^Firewall \(/ }).click();
      await page.getByRole('button', { name: 'Next evidence page', exact: true }).click();
      await expect(page.getByRole('button', { name: 'First evidence page', exact: true })).toBeVisible();
      await expect(page.getByRole('button', { name: 'Next evidence page', exact: true })).toHaveCount(0);
      await expect(page.getByRole('button', { name: `Firewall (${next.hostNetwork[0].observations.length})`, exact: true })).toBeVisible();
      await page.getByRole('button', { name: 'First evidence page', exact: true }).click();
      await expect(page.getByRole('button', { name: 'Next evidence page', exact: true })).toBeVisible();
      await expect(page.getByRole('button', { name: `Firewall (${first.hostNetwork[0].observations.length})`, exact: true })).toBeVisible();
      await expect(page.getByRole('alert')).toHaveCount(0);
    }
  } finally { await server.stop(); await rm(dir, { recursive: true, force: true }); }
});

test('workload detail opens while the enclosing list is still loading', async ({ page }) => {
  // A selected resource is independently readable. A slow enclosing page must
  // not delay its detail or replace it when the older list request completes.
  const dir = await mkdtemp(join(tmpdir(), 'omloom-detail-first-')); const server = await host(dir);
  let release;
  const held = new Promise(resolve => { release = resolve; });
  try {
    const { m, rpc } = await workspaceClient(server.url);
    const target = (await rpc({ oneofKind: 'write', write: m.Write.create({ kind: m.AssetKind.WORKLOAD, changes: [{ name: 'name', value: { oneofKind: 'text', text: 'Selected workload' } }] }) })).mutation.targets[0];
    await page.route('**/omloom.workspace.v1.Workspace/Call', async route => {
      const request = m.Request.fromBinary(route.request().postDataBuffer().subarray(5));
      if (request.command.oneofKind === 'query') await held;
      await route.continue();
    });
    await page.goto(`${server.url}/#/?view=4&asset=${target.assetId}&incarnation=${target.incarnation}`);
    await expect(page.getByRole('region', { name: 'Resource details' }).getByRole('heading', { name: 'Selected workload', exact: true })).toBeVisible();
    release();
    await expect(page.getByRole('button', { name: '↻ Refresh', exact: true })).toBeEnabled();
    await expect(page.getByRole('region', { name: 'Resource details' }).getByRole('heading', { name: 'Selected workload', exact: true })).toBeVisible();
  } finally { release(); await page.unrouteAll({ behavior: 'wait' }); await server.stop(); await rm(dir, { recursive: true, force: true }); }
});

test('saved mutation reports metadata refresh failure without an unhandled rejection', async ({ page }) => {
  // Only the post-save metadata request fails; the write still reaches the real host and disk.
  const dir = await mkdtemp(join(tmpdir(), 'omloom-save-refresh-'));
  const server = await host(dir);
  try {
    const { m } = await workspaceClient(server.url);
    const failures = [];
    page.on('pageerror', error => failures.push(error.message));
    let written = false; let interrupted = false;
    await page.route('**/omloom.workspace.v1.Workspace/Call', async route => {
      const request = m.Request.fromBinary(route.request().postDataBuffer().subarray(5));
      if (request.command.oneofKind === 'write') written = true;
      if (written && !interrupted && request.command.oneofKind === 'definitions') { interrupted = true; await route.abort('failed'); }
      else await route.continue();
    });
    await page.goto(server.url);
    await page.getByRole('button', { name: '+ Create resource', exact: true }).click();
    const dialog = page.getByRole('dialog');
    await dialog.getByLabel('Resource kind', { exact: true }).selectOption({ label: 'device' });
    await dialog.getByLabel('Name', { exact: true }).fill('Saved before refresh failure');
    await dialog.getByRole('button', { name: 'Save resource', exact: true }).click();
    await expect(dialog).toBeHidden();
    await expect(page.getByRole('alert')).toBeVisible();
    expect(interrupted).toBe(true);
    expect(failures).toEqual([]);
    await page.reload();
    await expect(page.getByRole('button', { name: 'Saved before refresh failure', exact: true })).toBeVisible();
  } finally { await server.stop(); await rm(dir, { recursive: true, force: true }); }
});

test('reload selection retries a failed query when its URL is unchanged', async ({ page }) => {
  // An identical route is not navigation; the explicit retry must still issue a real query.
  const dir = await mkdtemp(join(tmpdir(), 'omloom-query-retry-'));
  const server = await host(dir);
  try {
    const { m } = await workspaceClient(server.url);
    let queries = 0;
    await page.route('**/omloom.workspace.v1.Workspace/Call', async route => {
      const request = m.Request.fromBinary(route.request().postDataBuffer().subarray(5));
      if (request.command.oneofKind === 'query' && ++queries === 1) await route.abort('failed');
      else await route.continue();
    });
    await page.goto(server.url);
    await expect(page.getByRole('alert')).toBeVisible();
    await page.getByRole('button', { name: 'Reload selection', exact: true }).click();
    await expect(page.getByRole('alert')).toHaveCount(0);
    await expect(page.getByRole('heading', { name: 'Start with the infrastructure you know', exact: true })).toBeVisible();
    expect(queries).toBe(2);
  } finally { await server.stop(); await rm(dir, { recursive: true, force: true }); }
});

test('saved view capacity refusal preserves desktop and phone startup', async ({ page }) => {
  // Startup must remain usable after a real durable save reaches metadata capacity.
  const dir = await mkdtemp(join(tmpdir(), 'omloom-view-budget-'));
  const server = await host(dir);
  try {
    const { m, rpc } = await workspaceClient(server.url);
    const predicates = Array.from({ length: 8 }, () => ({ field: 'description', value: { name: 'description', value: { oneofKind: 'text', text: 'x'.repeat(950) } } }));
    let refused = false;
    for (let i = 0; i < 80; i++) {
      try { await rpc({ oneofKind: 'saveView', saveView: m.SaveView.create({ view: { name: `Large ${i}`, query: { columns: ['name'], predicates } } }) }); }
      catch (error) { expect(error.code).toBe('RESOURCE_EXHAUSTED'); refused = true; break; }
    }
    expect(refused).toBe(true);
    const metadata = await rpc({ oneofKind: 'views', views: {} });
    expect(m.Reply.toBinary({ result: metadata }).length).toBeLessThanOrEqual(512 * 1024);
    await rpc({ oneofKind: 'write', write: m.Write.create({ kind: m.AssetKind.DEVICE, changes: [{ name: 'name', value: { oneofKind: 'text', text: 'Readable inventory' } }] }) });
    for (const width of [1440, 390]) {
      await page.setViewportSize({ width, height: 900 });
      await page.goto(server.url); await page.reload();
      await expect(page.getByRole('button', { name: 'Readable inventory', exact: true })).toBeVisible();
      await expect(page.getByRole('alert')).toHaveCount(0);
    }
  } finally { await server.stop(); await rm(dir, { recursive: true, force: true }); }
});

test('definition capacity refusal preserves desktop and phone startup', async ({ page }) => {
  // Individually valid choice sets cannot strand accepted inventory behind oversized metadata.
  const dir = await mkdtemp(join(tmpdir(), 'omloom-schema-budget-'));
  const server = await host(dir);
  try {
    const { m, rpc } = await workspaceClient(server.url);
    let refused = false;
    for (let i = 0; i < 52; i++) {
      try {
        await rpc({ oneofKind: 'definition', definition: m.Definition.create({ name: `Choices ${i}`, type: m.FieldType.CHOICE, kinds: [m.AssetKind.DEVICE], choices: Array.from({ length: 64 }, (_, n) => String(n).padStart(3, '0') + 'x'.repeat(157)) }) });
      } catch (error) {
        expect(error.code).toBe('RESOURCE_EXHAUSTED');
        refused = true;
        break;
      }
    }
    expect(refused).toBe(true);
    const metadata = await rpc({ oneofKind: 'definitions', definitions: {} });
    expect(m.Reply.toBinary({ result: metadata }).length).toBeLessThanOrEqual(512 * 1024);
    await rpc({ oneofKind: 'write', write: m.Write.create({ kind: m.AssetKind.DEVICE, changes: [{ name: 'name', value: { oneofKind: 'text', text: 'Readable inventory' } }] }) });
    for (const width of [1440, 390]) {
      await page.setViewportSize({ width, height: 900 });
      await page.goto(server.url);
      await page.reload();
      await expect(page.getByRole('button', { name: 'Readable inventory', exact: true })).toBeVisible();
      await expect(page.getByRole('alert')).toHaveCount(0);
    }
  } finally { await server.stop(); await rm(dir, { recursive: true, force: true }); }
});

test('accepted wide columns remain browsable across byte-sized pages', async ({ page }) => {
  // Real binary RPC and disk fixtures prove the transport accepts every continuation.
  const dir = await mkdtemp(join(tmpdir(), 'omloom-wide-pages-'));
  const server = await host(dir);
  try {
    const { m, rpc } = await workspaceClient(server.url);
    const columns = ['name'];
    for (let i = 0; i < 11; i++) {
      const name = `Payload ${i}`;
      const result = await rpc({ oneofKind: 'definition', definition: m.Definition.create({ name, kinds: [m.AssetKind.DEVICE], type: m.FieldType.TEXT }) });
      columns.push(result.definitions.items.find(d => d.name === name).id);
    }
    for (let i = 0; i < 25; i++) await rpc({ oneofKind: 'write', write: m.Write.create({ kind: m.AssetKind.DEVICE, changes: columns.map(name => ({ name, value: { oneofKind: 'text', text: name === 'name' ? `wide-${String(i).padStart(2,'0')}` : 'x'.repeat(2048) } })) }) });
    await page.goto(`${server.url}/#/?columns=${columns.join(',')}`);
    await expect(page.getByRole('button', { name: 'wide-00', exact: true })).toBeVisible();
    await expect(page.getByRole('button', { name: 'Next page →', exact: true })).toBeEnabled();
    await page.getByRole('button', { name: 'Next page →', exact: true }).click();
    await expect(page.getByRole('button', { name: 'wide-24', exact: true })).toBeVisible();
    await expect(page.getByRole('button', { name: 'Next page →', exact: true })).toBeDisabled();
    await expect(page.getByRole('alert')).toHaveCount(0);
  } finally { await server.stop(); await rm(dir, { recursive: true, force: true }); }
});

test('mutations reset ordinary and local continuations on desktop and phone', async ({ page }) => {
  // A successful mutation changes the revision, so no old list or return cursor can survive it.
  test.setTimeout(90_000);
  const dir = await mkdtemp(join(tmpdir(), 'omloom-mutation-cursors-'));
  const server = await host(dir);
  try {
    const { m, rpc } = await workspaceClient(server.url);
    const csv = 'external_id,name\n' + Array.from({ length: 26 }, (_, i) => `${i},root-${String(i).padStart(2,'0')}`).join('\n');
    const preview = (await rpc({ oneofKind: 'import', import: m.Import.create({ source: 'cursor-fixture', kind: m.AssetKind.DEVICE, csv, mappings: [{ source: 'name', field: 'name' }] }) })).preview;
    const imported = await rpc({ oneofKind: 'import', import: m.Import.create({ preview: preview.id }) });
    const parent = imported.mutation.targets[0];
    for (let i = 0; i < 26; i++) await rpc({ oneofKind: 'write', write: m.Write.create({ kind: m.AssetKind.PORT, parent, changes: [{ name: 'name', value: { oneofKind: 'text', text: `port-${String(i).padStart(2,'0')}` } }] }) }, parent);
    for (const width of [1440, 390]) {
      await page.setViewportSize({ width, height: 900 });
      await page.goto(server.url);
      await page.getByRole('button', { name: 'Next page →', exact: true }).click();
      await expect(page).toHaveURL(/cursor=/);
      await expect(page.getByRole('button', { name: 'root-25', exact: true })).toBeVisible();
      await page.getByRole('checkbox', { name: width === 390 ? 'Select phone page' : 'Select visible page', exact: true }).check();
      await page.getByRole('button', { name: 'Review bulk change', exact: true }).click();
      const dialog = page.getByRole('dialog');
      await dialog.getByRole('combobox', { name: 'Change', exact: true }).selectOption('set');
      await dialog.getByLabel('Value', { exact: true }).fill(`Reviewed at ${width}`);
      await dialog.getByRole('button', { name: 'Apply reviewed change', exact: true }).click();
      await expect(dialog).toBeHidden();
      await expect(page).not.toHaveURL(/cursor=/);
      await expect(page.getByRole('alert')).toHaveCount(0);
      await page.getByRole('button', { name: 'Next page →', exact: true }).click();
      await page.getByRole('button', { name: 'root-25', exact: true }).click();
      await page.getByRole('button', { name: 'Edit resource', exact: true }).click();
      await dialog.getByRole('combobox', { name: 'Description change', exact: true }).selectOption('set');
      await dialog.getByLabel('Description', { exact: true }).fill(`Detail reviewed at ${width}`);
      await dialog.getByRole('button', { name: 'Save resource', exact: true }).click();
      await expect(dialog).toBeHidden();
      await expect(page.getByRole('region', { name: 'Resource details' })).toContainText(`Detail reviewed at ${width}`);
      await page.getByRole('button', { name: 'Close resource details', exact: true }).click();
      await expect(page).not.toHaveURL(/cursor=/);
      await expect(page.getByRole('alert')).toHaveCount(0);
      await expect(page.getByRole('button', { name: 'root-01', exact: true })).toBeVisible();
      await page.goto(`${server.url}/#/?asset=${parent.assetId}&incarnation=${parent.incarnation}&view=${m.View.HARDWARE}`);
      await expect(page.getByRole('region', { name: 'Resource details' })).toBeVisible();
      await expect(page.getByRole('button', { name: 'port-00', exact: true })).toBeVisible();
      const next = page.getByRole('button', { name: width === 390 ? 'Next local resources' : 'Next page →', exact: true });
      await expect(next).toBeEnabled();
      await next.click();
      await expect(page).toHaveURL(/localCursor=/);
      await expect(page.getByRole('button', { name: 'port-25', exact: true })).toBeVisible();
      await page.getByRole('button', { name: 'Edit resource', exact: true }).click();
      await dialog.getByRole('combobox', { name: 'Name change', exact: true }).selectOption('set');
      await dialog.getByLabel('Name', { exact: true }).fill(`Updated parent ${width}`);
      await dialog.getByRole('button', { name: 'Save resource', exact: true }).click();
      await expect(dialog).toBeHidden();
      await expect(page).not.toHaveURL(/localCursor=/);
      await expect(page.getByRole('region', { name: 'Resource details' }).getByRole('heading', { name: `Updated parent ${width}`, exact: true })).toBeVisible();
      await expect(page.getByRole('alert')).toHaveCount(0);
    }
  } finally { await server.stop(); await rm(dir, { recursive: true, force: true }); }
});

test('responsibility and account mutations discard enclosing page continuations', async ({ page }) => {
  // These panels mutate the same revision as resource edits; returning must never reuse an old cursor.
  test.setTimeout(90_000);
  for (const kindName of ['DEVICE','PROVIDER_ACCOUNT']) {
    const dir=await mkdtemp(join(tmpdir(),'omloom-panel-cursors-'));const server=await host(dir);
    try {
      const {m,rpc}=await workspaceClient(server.url);
      let parent;
      for(let i=0;i<26;i++) parent=(await rpc({oneofKind:'write',write:m.Write.create({kind:m.AssetKind[kindName],changes:[{name:'name',value:{oneofKind:'text',text:`Root ${String(i).padStart(2,'0')}`}}]})})).mutation.targets[0];
      if(kindName==='DEVICE') for(let i=0;i<26;i++) await rpc({oneofKind:'write',write:m.Write.create({kind:m.AssetKind.PORT,parent,changes:[{name:'name',value:{oneofKind:'text',text:`Port ${String(i).padStart(2,'0')}`}}]})},parent);
      for(const width of [1440,390]) {
        await page.setViewportSize({width,height:900});await page.goto(server.url);
        await page.getByRole('button',{name:'Next page →',exact:true}).click();
        await page.getByRole('button',{name:'Root 25',exact:true}).click();
        const dialog=page.getByRole('dialog');
        if(kindName==='DEVICE') {
          await page.getByRole('button',{name:'Edit claimed owner',exact:true}).click();
          await dialog.getByLabel('Party namespace',{exact:true}).fill('company.example');
          await dialog.getByLabel('Party identity',{exact:true}).fill('devops');
          await dialog.getByLabel('Party label',{exact:true}).fill(`DevOps ${width}`);
          await dialog.getByRole('button',{name:'Review assignment',exact:true}).click();
          await dialog.getByRole('button',{name:'Apply assignment',exact:true}).click();
        } else {
          await page.getByRole('button',{name:'Edit account metadata',exact:true}).click();
          if(width===1440) {
            await dialog.getByLabel('Provider namespace',{exact:true}).fill('provider.example');
            await dialog.getByLabel('Provider account identity',{exact:true}).fill('selected');
            await dialog.getByLabel('Account kind',{exact:true}).selectOption(String(m.ExternalAccountKind.ADMINISTRATION));
          }
          await dialog.getByLabel('Login label',{exact:true}).fill(`Selected ${width}`);
          await dialog.getByRole('button',{name:'Save account metadata',exact:true}).click();
        }
        await expect(dialog).toBeHidden();
        if(kindName==='DEVICE') await expect(page.getByRole('region',{name:'Ownership and responsibility'}).getByText(`DevOps ${width}`,{exact:true})).toBeVisible();
        else await expect(page.getByRole('region',{name:'External account',exact:true}).getByText(`Selected ${width}`,{exact:true})).toBeVisible();
        await page.getByRole('button',{name:'Close resource details',exact:true}).click();
        await expect(page).not.toHaveURL(/cursor=/);
        await expect(page.getByRole('alert')).toHaveCount(0);
        await expect(page.getByRole('button',{name:'Root 00',exact:true})).toBeVisible();
        if(kindName==='DEVICE') {
          await page.goto(`${server.url}/#/?view=${m.View.HARDWARE}&asset=${parent.assetId}&incarnation=${parent.incarnation}`);
          await expect(page.getByRole('button',{name:'Port 00',exact:true})).toBeVisible();
          await page.getByRole('button',{name:width===390?'Next local resources':'Next page →',exact:true}).click();
          await expect(page).toHaveURL(/localCursor=/);
          await expect(page.getByRole('button',{name:'Port 25',exact:true})).toBeVisible();
          await page.getByRole('button',{name:'Edit claimed owner',exact:true}).click();
          await dialog.getByLabel('Party label',{exact:true}).fill(`Local ${width}`);
          await dialog.getByRole('button',{name:'Review assignment',exact:true}).click();
          await dialog.getByRole('button',{name:'Apply assignment',exact:true}).click();
          await expect(dialog).toBeHidden();
          await expect(page).not.toHaveURL(/localCursor=/);
          await expect(page.getByRole('button',{name:'Port 00',exact:true})).toBeVisible();
          await expect(page.getByRole('alert')).toHaveCount(0);
        }
      }
    }finally{await server.stop();await rm(dir,{recursive:true,force:true});}
  }
});

test('reordering field applicability saves without a migration on desktop and phone', async ({ page }) => {
  // Checkbox interaction order cannot turn an unchanged applicability set into a value migration.
  const dir = await mkdtemp(join(tmpdir(), 'omloom-field-order-'));
  const server = await host(dir);
  try {
    for (const width of [1440, 390]) {
      await page.setViewportSize({ width, height: 900 });
      await page.goto(server.url);
      const fields = page.getByRole('button', { name: 'Company fields', exact: true });
      await expect(page.getByRole('button', { name: '+ Create resource', exact: true })).toBeVisible();
      if (!await fields.isVisible()) await page.getByRole('button', { name: 'More', exact: true }).click();
      await fields.click();
      const dialog = page.getByRole('dialog');
      const name = `Purpose ${width}`;
      await dialog.getByLabel('Name', { exact: true }).fill(name);
      await dialog.getByRole('checkbox', { name: 'device', exact: true }).check();
      await dialog.getByRole('checkbox', { name: 'virtual machine', exact: true }).check();
      await dialog.getByRole('button', { name: 'Review / save definition', exact: true }).click();
      await expect(dialog).toBeHidden();
      if (!await fields.isVisible()) await page.getByRole('button', { name: 'More', exact: true }).click();
      await fields.click();
      await dialog.getByRole('combobox', { name: 'Definition', exact: true }).selectOption({ label: name }, { timeout: 5000 });
      await dialog.getByRole('checkbox', { name: 'device', exact: true }).uncheck();
      await dialog.getByRole('checkbox', { name: 'device', exact: true }).check();
      await dialog.getByRole('button', { name: 'Review / save definition', exact: true }).click();
      await expect(dialog).toBeHidden();
    }
  } finally { await server.stop(); await rm(dir, { recursive: true, force: true }); }
});

test('loopback host rejects foreign authorities before API and static routes', async ({ page }) => {
  // A foreign browser origin must not obtain fixture authority by rebinding DNS to loopback.
  const dir = await mkdtemp(join(tmpdir(), 'omloom-authority-'));
  const server = await host(dir);
  try {
    const port = new URL(server.url).port;
    for (const authority of [`rebind.example:${port}`, `127.0.0.1:${port === '80' ? '81' : '80'}`, `user@127.0.0.1:${port}`]) {
      const headers = { Host: authority };
      expect((await page.request.get(server.url, { headers })).status()).toBe(421);
      expect((await page.request.post(`${server.url}/omloom.workspace.v1.Workspace/Call`, { headers, data: Buffer.alloc(5) })).status()).toBe(421);
    }
    for (const authority of [`127.0.0.1:${port}`, `localhost:${port}`]) {
      expect((await page.request.get(server.url, { headers: { Host: authority } })).status()).toBe(200);
    }
    const { connect } = await import('node:http2');
    const client = connect(server.url);
    try {
      for (const [authority, status] of [[`localhost:${port}`, 200], [`rebind.example:${port}`, 421]]) {
        const response = await new Promise((resolve, reject) => {
          const request = client.request({ ':path': '/', ':authority': authority });
          request.on('response', headers => resolve(headers[':status']));
          request.on('error', reject);
          request.resume();
          request.end();
        });
        expect(response).toBe(status);
      }
    } finally { client.close(); }
    for (const width of [1440, 390]) {
      await page.setViewportSize({ width, height: 900 });
      await page.goto(server.url);
      await create(page, `Accepted loopback resource ${width}`);
    }
  } finally { await server.stop(); await rm(dir, { recursive: true, force: true }); }
});

test('network rename preserves scopes beyond the bounded options window', async ({ page }) => {
  // Real disk/API fixtures place accepted references after 1,000 choices; rename must never clear them.
  test.setTimeout(120_000);
  const { buildSync } = await import('esbuild');
  const { GrpcWebFetchTransport } = await import('@protobuf-ts/grpcweb-transport');
  const built = buildSync({ stdin: { contents: 'export * from "./web/src/generated/omloom/v1/workspace"; export {AssetKind} from "./web/src/generated/omloom/v1/contracts";', resolveDir: process.cwd() }, bundle: true, write: false, format: 'esm', platform: 'node' });
  const m = await import('data:text/javascript;base64,' + Buffer.from(built.outputFiles[0].contents).toString('base64'));
  const dir = await mkdtemp(join(tmpdir(), 'omloom-network-options-'));
  const server = await host(dir);
  try {
    const transport = new GrpcWebFetchTransport({ baseUrl: server.url, format: 'binary' });
    async function rpc(command) {
      const q = await transport.unary(m.Workspace.methods[0], m.Request.create({ command: { oneofKind: 'query', query: m.Query.create({ limit: 1 }) } }), transport.mergeOptions());
      return (await transport.unary(m.Workspace.methods[0], m.Request.create({ command, working: { expected: q.response.result.page.revision } }), transport.mergeOptions())).response.result;
    }
    async function object(kind, name) {
      return (await rpc({ oneofKind: 'write', write: m.Write.create({ kind, changes: [{ name: 'name', value: { oneofKind: 'text', text: name } }] }) })).mutation.targets[0];
    }
    const space = await object(m.AssetKind.ADDRESS_SPACE, 'A namespace');
    for (let offset = 0; offset < 1000; offset += 100) {
      const csv = 'external_id,name\n' + Array.from({ length: 100 }, (_, i) => `${offset+i},A option ${String(offset+i).padStart(4,'0')}`).join('\n');
      const preview = (await rpc({ oneofKind:'import', import:m.Import.create({ source:'option-fixture', kind:m.AssetKind.VRF, csv, mappings:[{ source:'name', field:'name' }] }) })).preview;
      expect(preview.id).toBeDefined();
      expect(preview.rows.every(row => row.change !== undefined)).toBe(true);
      await rpc({ oneofKind:'import', import:m.Import.create({ preview:preview.id }) });
    }
    const link = await object(m.AssetKind.L2_DOMAIN, 'Z selected L2');
    const routing = await object(m.AssetKind.VRF, 'Z selected VRF');
    const target = (await rpc({ oneofKind:'network', network:m.Network.create({ kind:m.AssetKind.IP_ADDRESS, name:'Scoped address', cidr:'192.0.2.10/32', addressSpace:space, link, routing, sharing:m.SharedUse.EXCLUSIVE }) })).mutation.targets[0];
    const url = `${server.url}/#/?view=1&asset=${target.assetId}&incarnation=${target.incarnation}`;
    for (const width of [1440, 390]) {
      await page.setViewportSize({ width, height:900 });
      await page.goto(url);
      await page.getByRole('button', { name:'Review network settings', exact:true }).click();
      const dialog = page.getByRole('dialog');
      await expect(dialog).toContainText('Showing a bounded selection');
      const name = `Renamed address ${width}`;
      await dialog.getByLabel('Name', { exact:true }).fill(name);
      await dialog.getByRole('button', { name:'Apply reviewed settings', exact:true }).click();
      await expect(dialog).toBeHidden();
      const item = (await rpc({ oneofKind:'read', read:m.Read.create({ target }) })).detail.item;
      const field = key => item.fields.find(f => f.name === key)?.value.text;
      expect(field('name')).toBe(name);
      expect(field('link_scope')).toBe(`${link.assetId}/${link.incarnation}`);
      expect(field('routing_context')).toBe(`${routing.assetId}/${routing.incarnation}`);
    }
  } finally { await server.stop(); await rm(dir, { recursive:true, force:true }); }
});

test('local network evidence keeps service ownership, coverage and phone navigation', async ({ page }) => {
  // Actual API/storage setup exercises the same inspector on desktop and phone, without personal fixtures.
  const { buildSync } = await import('esbuild');
  const { GrpcWebFetchTransport } = await import('@protobuf-ts/grpcweb-transport');
  const built = buildSync({ stdin: { contents: 'export * from "./web/src/generated/omloom/v1/workspace"; export {AssetKind} from "./web/src/generated/omloom/v1/contracts";', resolveDir: process.cwd() }, bundle: true, write: false, format: 'esm', platform: 'node' });
  const m = await import('data:text/javascript;base64,' + Buffer.from(built.outputFiles[0].contents).toString('base64'));
  const dir = await mkdtemp(join(tmpdir(), 'omloom-network-evidence-'));
  const server = await host(dir);
  try {
    const transport = new GrpcWebFetchTransport({ baseUrl: server.url, format: 'binary' });
    async function rpc(command, focus) {
      const q = await transport.unary(m.Workspace.methods[0], m.Request.create({ command: { oneofKind: 'query', query: m.Query.create({ limit: 1 }) } }), transport.mergeOptions());
      const { response } = await transport.unary(m.Workspace.methods[0], m.Request.create({ command, working: { roots: [], focus, expected: q.response.result.page.revision } }), transport.mergeOptions());
      return response.result;
    }
    async function object(kind, name, parent) {
      const result = await rpc({ oneofKind: 'write', write: m.Write.create({ kind, parent, changes: [{ name: 'name', value: { oneofKind: 'text', text: name } }] }) }, parent);
      return result.mutation.targets[0];
    }
    const device = await object(m.AssetKind.DEVICE, 'Observed host');
    const os = await object(m.AssetKind.SOFTWARE_INSTALLATION, 'Observed OS', device);
    const service = await object(m.AssetKind.SERVICE, 'Observed tunnel', os);
    const tunnel = await object(m.AssetKind.INTERFACE, 'tunnel-local', service);
    await rpc({oneofKind:'interfaceAddresses',interfaceAddresses:m.InterfaceAddresses.create({interface:tunnel,source:'Fixture collector',observedAt:1000n,complete:true,addresses:[{address:'192.0.2.50',stability:m.AddressStability.STABLE,state:m.AddressState.PREFERRED}]})},tunnel);
    const otherHost = await object(m.AssetKind.DEVICE, 'Other host');
    const otherOS = await object(m.AssetKind.SOFTWARE_INSTALLATION, 'Other OS', otherHost);
    const otherService = await object(m.AssetKind.SERVICE, 'Observed tunnel', otherOS);
    const otherInterface = await object(m.AssetKind.INTERFACE, 'other-tunnel', otherService);
    await object(m.AssetKind.PORT, 'Direct physical port', device);
    const board = await object(m.AssetKind.DEVICE, 'Installed board', device);
    await object(m.AssetKind.PORT, 'Nested physical port', board);
    await rpc({oneofKind:'interfaceAddresses',interfaceAddresses:m.InterfaceAddresses.create({interface:otherInterface,source:'Fixture collector',observedAt:1000n,complete:true,addresses:[{address:'198.51.100.50',stability:m.AddressStability.STABLE,state:m.AddressState.PREFERRED}]})},otherInterface);
    // Ordinary browsing selects a level on the server, including count and pagination.
    // Opening one host must not materialize components of other hosts or deeper boards.
    for (const width of [1440, 390]) {
      await page.setViewportSize({width,height:900});
      await page.goto(`${server.url}/#/?view=2`);
      await expect(page.getByRole('button',{name:'Observed host',exact:true})).toBeVisible();
      await expect(page.getByRole('button',{name:'Other host',exact:true})).toBeVisible();
      await expect(page.locator('tbody')).not.toContainText('Direct physical port');
      await expect(page.locator('tbody')).not.toContainText('Installed board');
      await expect(page.locator('tbody')).not.toContainText('Nested physical port');
      await page.getByRole('button',{name:'Observed host',exact:true}).click();
      await page.getByRole('button',{name:'Use as scope',exact:true}).click();
      await expect(page.locator('tbody')).toContainText('Direct physical port');
      await expect(page.locator('tbody')).toContainText('Installed board');
      await expect(page.locator('tbody')).not.toContainText('Nested physical port');
      await expect(page.locator('tbody')).not.toContainText('Other host');
      const requested = await rpc({oneofKind:'query',query:m.Query.create({view:m.View.HARDWARE,levelOnly:true,roots:[device],limit:25,includeContext:true})});
      expect(requested.page.total).toBe(2n);
      expect(requested.page.contexts.every(c=>c.addresses.length===0)).toBe(true);
    }
    // An unaddressed interface must not borrow another interface's addresses from its OS.
    const osInterface=await object(m.AssetKind.INTERFACE,'addressed sibling',os);
    await object(m.AssetKind.INTERFACE,'unaddressed sibling',os);
    await rpc({oneofKind:'interfaceAddresses',interfaceAddresses:m.InterfaceAddresses.create({interface:osInterface,source:'Fixture collector',observedAt:1000n,complete:true,addresses:[{address:'192.0.2.99',stability:m.AddressStability.STABLE,state:m.AddressState.PREFERRED}]})},osInterface);
    await page.goto(`${server.url}/#/?view=1&search=unaddressed`);
    const unaddressed=page.locator('tbody tr').filter({has:page.getByRole('button',{name:'unaddressed sibling',exact:true})});
    await expect(unaddressed).toContainText('Address not known');
    await expect(unaddressed).not.toContainText('192.0.2.99');
    await expect(unaddressed).not.toContainText('Host address');
    await object(m.AssetKind.VIRTUAL_MACHINE, 'Fixture guest');
    await page.goto(`${server.url}/#/?view=3`);
    await expect(page.getByRole('button', { name: 'Fixture guest', exact: true })).toBeVisible();
    await expect(page.getByRole('button', { name: 'Observed host', exact: true })).toHaveCount(0);
    await expect(page.getByRole('button', { name: 'Other host', exact: true })).toHaveCount(0);
    const categories=page.getByRole('navigation',{name:'Resource categories'});
    await categories.getByRole('button',{name:'Hypervisors',exact:true}).click();
    await expect(page.getByRole('button',{name:'Fixture guest',exact:true})).toHaveCount(0);
    await page.getByRole('button',{name:'+ Create resource',exact:true}).click();
    await expect(page.getByRole('dialog').getByLabel('Resource kind',{exact:true}).locator('option:checked')).toHaveText('Hypervisor (OS installation)');
    await expect(page.getByRole('dialog').getByLabel('Virtualization role',{exact:true})).toHaveValue('VIRTUALIZATION_ROLE_HYPERVISOR');
    await page.getByRole('dialog').getByRole('button',{name:'Cancel',exact:true}).click();
    await categories.getByRole('button',{name:'Virtual machines',exact:true}).click();
    await expect(page.getByRole('button',{name:'Fixture guest',exact:true})).toBeVisible();
    await page.reload();
    await expect(categories.getByRole('button',{name:'Virtual machines',exact:true})).toHaveAttribute('aria-pressed','true');
    await page.goto(`${server.url}/#/?view=4&search=Observed%20tunnel`);
    await expect(page.getByRole('button', { name: 'Observed tunnel', exact: true })).toHaveCount(2);
    await expect(page.getByRole('button', { name: 'Open owner Observed host / Observed OS', exact: true })).toBeVisible();
    await expect(page.getByRole('button', { name: 'Open owner Other host / Other OS', exact: true })).toBeVisible();
    await expect(page.getByText('Address: 192.0.2.50', { exact: true })).toBeVisible();
    await expect(page.getByText('Address: 198.51.100.50', { exact: true })).toBeVisible();
    await page.goto(`${server.url}/#/?asset=${tunnel.assetId}&incarnation=${tunnel.incarnation}`);
    const current=page.getByRole('region',{name:'Current interface addresses'});
    await expect(current).toContainText('192.0.2.50');
    await expect(current).toContainText('stable · preferred');
    // A partial collection must leave the first address's date intact in the actual UI.
    await rpc({oneofKind:'interfaceAddresses',interfaceAddresses:m.InterfaceAddresses.create({interface:tunnel,source:'Fixture collector',observedAt:2000n,complete:false,addresses:[{address:'192.0.2.51',stability:m.AddressStability.STABLE,state:m.AddressState.PREFERRED}]})},tunnel);
    await page.reload();
    await expect(current.locator('li').filter({hasText:'192.0.2.50'}).locator('time')).toHaveAttribute('datetime',new Date(1000).toISOString());
    await expect(current.locator('li').filter({hasText:'192.0.2.51'}).locator('time')).toHaveAttribute('datetime',new Date(2000).toISOString());
    const listener=await object(m.AssetKind.SERVICE,'Plain listener',os);
    await rpc({ oneofKind: 'hostNetwork', hostNetwork: m.HostNetwork.create({ installation: os, source: 'Test collection', observedAt: 1000n, basis: 'test-boot', socketCoverage: 'Kernel verified; process coverage partial', routeCoverage: 'Local IPv4 table', applicationCoverage: 'verified', packetFilterCoverage: 'not run or blocked: permission denied', firewallEnabled: true, blockAll: false, sockets: [
      { service, protocol: 'udp', family: 'IPv4', bind: '*', port: 51820 },
      { service, protocol: 'tcp', family: 'IPv4', bind: '127.0.0.1', port: 9000 },
      { service:listener, protocol:'tcp', family:'IPv4', bind:'192.0.2.60', port:8080 },
      { service, protocol:'udp', family:'IPv6', bind:'*', port:51821 },
    ], routes: [{ destination: 'default', gateway: '192.0.2.1', interface: 'en0', flags: 'UGSc', family: 'IPv4', linkLayer: false }], rules: [{ subject: '/Applications/Example.app', action: 'allow incoming' }] }) }, os);
    const address = `${server.url}/#/?asset=${os.assetId}&incarnation=${os.incarnation}`;
    for (const width of [1440, 390]) {
      await page.setViewportSize({ width, height: 900 });
      await page.goto(`${server.url}/#/?view=4&search=Plain%20listener`);
      const listenerRow=page.locator('tbody tr').filter({has:page.getByRole('button',{name:'Plain listener',exact:true})});
      await expect(listenerRow).toContainText('192.0.2.60:8080/tcp');
      await expect(listenerRow).toContainText('Observed host / Observed OS');
      await page.goto(address);
      const section = page.getByRole('region', { name: 'Local network observations', exact: true });
      await expect(section).toContainText('*:51820');
      await expect(section).toContainText('*:51821');
      await expect(section).not.toContainText('[*]');
      const ownerBox = await section.getByRole('button', { name: 'Observed tunnel', exact: true }).first().boundingBox();
      expect(ownerBox.width).toBeGreaterThan(100);
      expect(ownerBox.height).toBeLessThan(80);
      await expect(section).not.toContainText('127.0.0.1:9000');
      await expect(section.getByLabel('Non-loopback binds only')).toHaveCount(0);
      await expect(section).not.toContainText('127.0.0.1:9000');
      await section.getByRole('button', { name: 'Routes (1)', exact: true }).click();
      await expect(section).toContainText('192.0.2.1');
      await section.getByRole('button', { name: 'Firewall (1)', exact: true }).click();
      await expect(section).toContainText('permission denied');
      await expect(section).toContainText('Effective per-port decision: unknown');
      await expect(section).toContainText('/Applications/Example.app');
      await section.getByRole('button', { name: 'Endpoints (3)', exact: true }).click();
      await section.getByRole('button', { name: 'Observed tunnel', exact: true }).first().click();
      await expect(page.getByRole('heading', { name: 'Observed tunnel', exact: true })).toBeVisible();
      await expect(page.getByRole('region', { name: 'Resource details' })).toContainText('tunnel-local');
      await expect(section.getByRole('button', { name: /^Routes/ })).toHaveCount(0);
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
      await page.screenshot({path:`test-results/network-service-${width}.png`,fullPage:true});
    }
  } finally { await server.stop(); await rm(dir, { recursive: true, force: true }); }
});

test('phone creates local components and returns through the original context', async ({ page }) => {
  // Child creation must not become a global peer or require upstream network knowledge.
  const dir = await mkdtemp(join(tmpdir(), 'omloom-local-phone-'));
  const server = await host(dir);
  try {
    page.setDefaultTimeout(5000);
    await page.setViewportSize({ width: 390, height: 844 });
    await page.goto(server.url);
    const vm = await create(page, 'Guest under review', 'VIRTUAL_MACHINE');
    await page.getByLabel('Resource perspective', { exact: true }).selectOption({ label: 'Network' }, { timeout: 5000 });
    await expect(page.getByRole('button', { name: 'Allocate IP / prefix', exact: true })).toHaveCount(0);
    await page.getByRole('button', { name: '+ Create resource', exact: true }).click();
    const dialog = page.getByRole('dialog');
    await expect(dialog).toContainText('Inside Guest under review');
    await expect(dialog.getByLabel('Resource kind', { exact: true }).locator('option').filter({ hasText: /^virtual machine$/ })).toHaveCount(0);
    await dialog.getByLabel('Resource kind', { exact: true }).selectOption({ label: 'interface' });
    await dialog.getByLabel('Name', { exact: true }).fill('eth0');
    await dialog.getByRole('button', { name: 'Save resource', exact: true }).click();
    await expect(dialog).toBeHidden();
    await expect(page.getByRole('region', { name: 'Resource details' })).toContainText('eth0');
    await page.getByRole('button', { name: '+ Create resource', exact: true }).click();
    await dialog.getByLabel('Resource kind', { exact: true }).selectOption({ label: 'address reference' });
    await dialog.getByLabel('IP address', { exact: true }).fill('192.0.2.10');
    await expect(dialog.getByLabel('Address space', { exact: true })).toHaveCount(0);
    await dialog.getByRole('button', { name: 'Save known address', exact: true }).click();
    await expect(dialog).toBeHidden();
    await expect(page.getByRole('region', { name: 'Resource details' })).toContainText('192.0.2.10');
    await page.reload();
    await expect(page.getByRole('region', { name: 'Resource details' })).toContainText('192.0.2.10');
    await page.getByRole('button', { name: 'Close resource details', exact: true }).click();
    await expect(page.getByRole('region', { name: 'Resource details' })).toContainText('eth0');
    await page.getByRole('button', { name: 'Close resource details', exact: true }).click();
    expect(new URLSearchParams(new URL(page.url()).hash.split('?')[1]).get('asset')).toBe(vm.asset_id);
    await expect(page.getByRole('region', { name: 'Resource details' })).toContainText('Guest under review');
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
    await page.screenshot({ path: 'test-results/local-components-phone.png', fullPage: true });
  } finally { await server.stop(); await rm(dir, { recursive: true, force: true }); }
});

test('inventory choices apply together, cancel cleanly and keep the list compact', async ({ page }) => {
  // Native multi-selects clipped options and consumed the search area; choices need a deliberate review step.
  const dir = await mkdtemp(join(tmpdir(), 'omloom-choices-ui-'));
  const server = await host(dir);
  try {
    await page.setViewportSize({ width: 1024, height: 768 });
    await page.goto(server.url);
    await create(page, 'Rack host');
    await create(page, 'Build guest', 'VIRTUAL_MACHINE');
    await page.getByRole('button', { name: 'Close resource details', exact: true }).click();
    await page.getByRole('button', { name: 'Resource kinds', exact: true }).click();
    const dialog = page.getByRole('dialog');
    await dialog.getByRole('checkbox', { name: 'device', exact: true }).check();
    await dialog.getByRole('button', { name: 'Cancel', exact: true }).click();
    await expect(page.getByRole('button', { name: 'Build guest', exact: true })).toBeVisible();
    await page.getByRole('button', { name: 'Resource kinds', exact: true }).click();
    await dialog.getByRole('checkbox', { name: 'device', exact: true }).check();
    await dialog.getByRole('button', { name: 'Apply selection', exact: true }).click();
    await expect(page.getByRole('button', { name: 'Build guest', exact: true })).toHaveCount(0);
    await expect(page.getByRole('button', { name: 'Rack host', exact: true })).toBeVisible();
    await page.getByRole('button', { name: 'Columns', exact: true }).click();
    await dialog.getByRole('checkbox', { name: 'Serial number', exact: true }).check();
    await dialog.getByRole('button', { name: 'Apply selection', exact: true }).click();
    await expect(page.getByRole('columnheader', { name: 'serial', exact: true })).toBeVisible();
    await page.getByRole('button', { name: 'Columns', exact: true }).click();
    for (const name of ['Name', 'Description', 'Serial number']) await dialog.getByRole('checkbox', { name, exact: true }).uncheck();
    await expect(dialog.getByRole('button', { name: 'Apply selection', exact: true })).toBeDisabled();
    await page.keyboard.press('Escape');
    await expect(page.getByRole('button', { name: 'Columns', exact: true })).toBeFocused();
    await expect(page.locator('select[multiple]')).toHaveCount(0);
    const searchBox = await page.getByLabel('Search inventory').boundingBox();
    const table = await page.locator('table').boundingBox();
    expect(searchBox.width).toBeGreaterThan(280);
    expect(table.y).toBeLessThan(380);
    await page.setViewportSize({ width: 1440, height: 900 });
    await page.getByRole('button', { name: 'Rack host', exact: true }).click();
    await expect(page.getByRole('region', { name: 'Resource details' })).toBeVisible();
    const inventory = await page.getByRole('region', { name: 'Inventory', exact: true }).boundingBox();
    const detail = await page.getByRole('region', { name: 'Resource details' }).boundingBox();
    expect(detail.x).toBeGreaterThanOrEqual(inventory.x + inventory.width);
    await page.screenshot({ path: 'test-results/inventory-wide-inspector.png', fullPage: true });
  } finally { await server.stop(); await rm(dir, { recursive: true, force: true }); }
});

test('broader connections page beyond 25 peers and remove a later relationship', async ({ page }) => {
  // A focused inspector hides peer links; the broader dialog must expose every authorized page.
  const { buildSync } = await import('esbuild');
  const { GrpcWebFetchTransport } = await import('@protobuf-ts/grpcweb-transport');
  const built=buildSync({stdin:{contents:'export * from "./web/src/generated/omloom/v1/workspace"; export {AssetKind,RelationKind} from "./web/src/generated/omloom/v1/contracts";',resolveDir:process.cwd()},bundle:true,write:false,format:'esm',platform:'node'});
  const m=await import('data:text/javascript;base64,'+Buffer.from(built.outputFiles[0].contents).toString('base64'));
  const dir=await mkdtemp(join(tmpdir(),'omloom-peer-pages-')); const server=await host(dir);
  try {
    const transport=new GrpcWebFetchTransport({baseUrl:server.url,format:'binary'});
    async function rpc(command) {
      const q=(await transport.unary(m.Workspace.methods[0],m.Request.create({command:{oneofKind:'query',query:m.Query.create({limit:1})}}),transport.mergeOptions())).response.result;
      return (await transport.unary(m.Workspace.methods[0],m.Request.create({command,working:{expected:q.page.revision}}),transport.mergeOptions())).response.result;
    }
    async function object(name) { return (await rpc({oneofKind:'write',write:m.Write.create({kind:m.AssetKind.SERVICE,changes:[{name:'name',value:{oneofKind:'text',text:name}}]})})).mutation.targets[0]; }
    const anchor=await object('Application');
    for(let i=0;i<30;i++) {
      const peer=await object(`Peer ${i}`);
      await rpc({oneofKind:'relation',relation:m.Relationship.create({relation:{from:anchor,to:peer,kind:m.RelationKind.DEPENDS_ON}})});
    }
    for(const [index,width] of [1440,390].entries()) {
      const first=(await rpc({oneofKind:'read',read:m.Read.create({target:anchor})})).detail;
      expect(first.relations).toHaveLength(25);
      const second=(await rpc({oneofKind:'read',read:m.Read.create({target:anchor,relationsCursor:first.relationsNext})})).detail;
      const relation=second.relations[0];
      const item=second.related.find(i=>i.target.assetId===relation.to.assetId);
      const name=item.fields.find(f=>f.name==='name').value.text;
      await page.setViewportSize({width,height:900});
      await page.goto(`${server.url}/#/?asset=${anchor.assetId}&incarnation=${anchor.incarnation}`);
      await page.getByRole('button',{name:'Connections in broader scope',exact:true}).click();
      const dialog=page.getByRole('dialog',{name:'Connections in broader scope',exact:true});
      await dialog.getByRole('button',{name:'Review existing connections',exact:true}).click();
      await expect(dialog.locator('.connection-list li')).toHaveCount(25);
      await expect(dialog.getByRole('button',{name:'Next connections',exact:true})).toBeVisible();
      await dialog.getByRole('button',{name:'Next connections',exact:true}).click();
      await expect(dialog.locator('.connection-list li')).toHaveCount(5-index);
      await dialog.locator('.connection-list li').filter({has:page.getByText(name,{exact:true})}).getByRole('button',{name:'Review removal',exact:true}).click();
      await dialog.getByRole('button',{name:'Remove reviewed relationship',exact:true}).click();
      await expect(dialog).toBeHidden();
      const remaining=(await rpc({oneofKind:'read',read:m.Read.create({target:anchor,relationsCursor:undefined})})).detail;
      const last=(await rpc({oneofKind:'read',read:m.Read.create({target:anchor,relationsCursor:remaining.relationsNext})})).detail;
      const all=[...remaining.relations,...last.relations];
      expect(all).toHaveLength(29-index);
      expect(all.some(r=>r.relationId===relation.relationId)).toBe(false);
    }
  } finally { await server.stop(); await rm(dir,{recursive:true,force:true}); }
});

test('visible relationship removal keeps its endpoints during navigation on desktop and phone', async ({ page }) => {
  // A pending detail navigation must not change the context of an action still shown on screen.
  const dir = await mkdtemp(join(tmpdir(), 'omloom-relation-navigation-'));
  const server = await host(dir);
  let release;
  try {
    const { m, rpc } = await workspaceClient(server.url);
    for (const width of [1440, 390]) {
      const parent = (await rpc({ oneofKind: 'write', write: m.Write.create({ kind: m.AssetKind.DEVICE, changes: [{ name: 'name', value: { oneofKind: 'text', text: `Parent ${width}` } }] }) })).mutation.targets[0];
      const child = (await rpc({ oneofKind: 'write', write: m.Write.create({ parent, kind: m.AssetKind.PORT, changes: [{ name: 'name', value: { oneofKind: 'text', text: `Port ${width}` } }] }) }, parent)).mutation.targets[0];
      let held = false;
      const gate = new Promise(resolve => { release = resolve; });
      await page.route('**/omloom.workspace.v1.Workspace/Call', async route => {
        const bytes = route.request().postDataBuffer();
        const command = bytes ? m.Request.fromBinary(bytes.subarray(5)).command : undefined;
        if (!held && command?.oneofKind === 'read' && command.read.target?.assetId === child.assetId) { held = true; await gate; }
        await route.continue();
      });
      await page.setViewportSize({ width, height: 900 });
      await page.goto(`${server.url}/#/?asset=${parent.assetId}&incarnation=${parent.incarnation}`);
      const details = page.getByRole('region', { name: 'Resource details' });
      await details.getByRole('button', { name: `has port · Port ${width}`, exact: true }).click();
      await expect.poll(() => held).toBe(true);
      await expect(details.getByRole('heading', { name: `Parent ${width}`, exact: true })).toBeVisible();
      await details.getByRole('button', { name: 'Remove relationship', exact: true }).click();
      await expect(details.getByRole('heading', { name: `Port ${width}`, exact: true })).toBeVisible();
      await expect(page.getByRole('alert')).toHaveCount(0);
      const result = await rpc({ oneofKind: 'read', read: m.Read.create({ target: parent }) });
      expect(result.detail.relations).toHaveLength(0);
      release(); release = undefined;
      await page.unrouteAll({ behavior: 'wait' });
    }
  } finally { release?.(); await page.unrouteAll({ behavior: 'wait' }); await server.stop(); await rm(dir, { recursive: true, force: true }); }
});

test('peer connection review preserves the resource context through edit and removal', async ({ page }) => {
  // Local details must never silently expose peers or edit an unseen old endpoint.
  const dir = await mkdtemp(join(tmpdir(), 'omloom-peer-ui-')); const server = await host(dir);
  try {
    await page.goto(server.url);
    await create(page,'Application','SERVICE');
    const backup = await create(page,'Backup','SERVICE');
    const archive = await create(page,'Archive','SERVICE');
    await page.getByRole('button',{name:'Close resource details',exact:true}).click();
    await page.getByLabel('Search inventory').fill('Application'); await page.getByRole('button',{name:'Search',exact:true}).click();
    await page.getByRole('button',{name:'Application',exact:true}).click(); const original = page.url();
    await page.getByRole('button',{name:'Connections in broader scope',exact:true}).click();
    const dialog = page.getByRole('dialog');
    await dialog.getByRole('button',{name:'Choose connection endpoints',exact:true}).click();
    await dialog.getByLabel('Other endpoint',{exact:true}).selectOption(`${backup.asset_id}/${backup.incarnation}`);
    await dialog.getByRole('button',{name:'Review connection',exact:true}).click();
    await dialog.getByLabel('Relationship',{exact:true}).selectOption({label:'depends on'});
    await dialog.getByRole('button',{name:'Back',exact:true}).click();
    await expect(dialog.getByLabel('Other endpoint',{exact:true})).toHaveValue(`${backup.asset_id}/${backup.incarnation}`);
    await dialog.getByRole('button',{name:'Review connection',exact:true}).click();
    await dialog.getByRole('button',{name:'Add relationship',exact:true}).click(); await expect(dialog).toBeHidden();
    await expect(page).toHaveURL(original);
    await expect(page.getByRole('region',{name:'Resource details'})).not.toContainText('Backup');
    await page.getByRole('button',{name:'Connections in broader scope',exact:true}).click();
    await dialog.getByRole('button',{name:'Review existing connections',exact:true}).click();
    await expect(dialog).toContainText('Backup');
    await dialog.getByRole('button',{name:'Change connection',exact:true}).click();
    await dialog.getByLabel('Other endpoint',{exact:true}).selectOption(`${archive.asset_id}/${archive.incarnation}`);
    await dialog.getByRole('button',{name:'Review connection',exact:true}).click();
    await dialog.getByRole('button',{name:'Save reviewed relationship',exact:true}).click(); await expect(dialog).toBeHidden();
    await page.getByRole('button',{name:'Connections in broader scope',exact:true}).click();
    await dialog.getByRole('button',{name:'Review existing connections',exact:true}).click();
    await expect(dialog).toContainText('Archive'); await expect(dialog).not.toContainText('Backup');
    await dialog.getByRole('button',{name:'Review removal',exact:true}).click();
    await dialog.getByRole('button',{name:'Remove reviewed relationship',exact:true}).click(); await expect(dialog).toBeHidden();
    await page.reload(); await expect(page).toHaveURL(original);
    await page.getByRole('button',{name:'Connections in broader scope',exact:true}).click();
    await dialog.getByRole('button',{name:'Review existing connections',exact:true}).click();
    await expect(dialog).toContainText('No connections are visible');
    await page.screenshot({path:'test-results/peer-context-desktop.png',fullPage:true});
  } finally { await server.stop(); await rm(dir,{recursive:true,force:true}); }
});

test('assigned allocations identify interfaces and services on desktop and phone', async ({ page }) => {
  // A real participant assignment must supply row hints without an address reference or observation.
  const { buildSync } = await import('esbuild');
  const { GrpcWebFetchTransport } = await import('@protobuf-ts/grpcweb-transport');
  const built = buildSync({ stdin:{ contents:'export * from "./web/src/generated/omloom/v1/workspace"; export {AssetKind} from "./web/src/generated/omloom/v1/contracts";', resolveDir:process.cwd() }, bundle:true, write:false, format:'esm', platform:'node' });
  const m = await import('data:text/javascript;base64,' + Buffer.from(built.outputFiles[0].contents).toString('base64'));
  const dir = await mkdtemp(join(tmpdir(), 'omloom-assignment-hints-')); const server = await host(dir);
  try {
    const transport = new GrpcWebFetchTransport({ baseUrl:server.url, format:'binary' });
    async function rpc(command, focus) {
      const q = (await transport.unary(m.Workspace.methods[0], m.Request.create({command:{oneofKind:'query',query:m.Query.create({limit:1})}}),transport.mergeOptions())).response.result;
      return (await transport.unary(m.Workspace.methods[0],m.Request.create({command,working:{focus,expected:q.page.revision}}),transport.mergeOptions())).response.result;
    }
    async function object(kind,name,parent) { return (await rpc({oneofKind:'write',write:m.Write.create({kind,parent,changes:[{name:'name',value:{oneofKind:'text',text:name}}]})},parent)).mutation.targets[0]; }
    const space=await object(m.AssetKind.ADDRESS_SPACE,'Assignment namespace');
    const service=await object(m.AssetKind.SERVICE,'Assigned service');
    const iface=await object(m.AssetKind.INTERFACE,'Assigned interface',service);
    const address=(await rpc({oneofKind:'network',network:m.Network.create({kind:m.AssetKind.IP_ADDRESS,name:'Canonical allocation',cidr:'192.0.2.51/32',addressSpace:space,sharing:m.SharedUse.EXCLUSIVE})})).mutation.targets[0];
    const detail=(await rpc({oneofKind:'read',read:m.Read.create({target:address})})).detail;
    await rpc({oneofKind:'member',member:m.Member.create({address,interface:iface,expected:detail.item.revision})});
    for (const parent of [iface,service]) {
      await rpc({oneofKind:'address',address:m.Address.create({parent,address:'203.0.113.44',meaning:m.AddressMeaning.DESTINATION})},parent);
    }
    for (const width of [1440,390]) {
      await page.setViewportSize({width,height:900});
      for (const name of ['Assigned service','Assigned interface']) {
        await page.goto(`${server.url}/#/?view=0&search=${encodeURIComponent(name)}`);
        const row=page.getByRole('row').filter({has:page.getByRole('button',{name,exact:true})});
        await expect(row).toContainText('192.0.2.51');
        await expect(row).not.toContainText('203.0.113.44');
        await page.getByRole('button',{name,exact:true}).click();
        await expect(page.getByRole('region',{name:'Resource details'}).getByRole('heading',{name,exact:true})).toBeVisible();
      }
    }
  } finally { await server.stop(); await rm(dir,{recursive:true,force:true}); }
});

test('VRRP reconciliation submits intended priority for existing and prepared allocations', async ({ page }) => {
  // All three reconciliation paths commit real membership, including a prepared address in an existing group.
  const { buildSync } = await import('esbuild');
  const { GrpcWebFetchTransport } = await import('@protobuf-ts/grpcweb-transport');
  const built = buildSync({ stdin:{ contents:'export * from "./web/src/generated/omloom/v1/workspace"; export {AssetKind} from "./web/src/generated/omloom/v1/contracts";', resolveDir:process.cwd() }, bundle:true, write:false, format:'esm', platform:'node' });
  const m = await import('data:text/javascript;base64,' + Buffer.from(built.outputFiles[0].contents).toString('base64'));
  const dir = await mkdtemp(join(tmpdir(), 'omloom-vrrp-binding-')); const server = await host(dir);
  try {
    const transport = new GrpcWebFetchTransport({ baseUrl:server.url, format:'binary' });
    async function rpc(command) {
      const q = (await transport.unary(m.Workspace.methods[0], m.Request.create({command:{oneofKind:'query',query:m.Query.create({limit:1})}}),transport.mergeOptions())).response.result;
      return (await transport.unary(m.Workspace.methods[0],m.Request.create({command,working:{expected:q.page.revision}}),transport.mergeOptions())).response.result;
    }
    async function object(kind,name) { return (await rpc({oneofKind:'write',write:m.Write.create({kind,changes:[{name:'name',value:{oneofKind:'text',text:name}}]})})).mutation.targets[0]; }
    const space=await object(m.AssetKind.ADDRESS_SPACE,'VRRP namespace');
    const link=await object(m.AssetKind.L2_DOMAIN,'VRRP link');
    const iface=await object(m.AssetKind.INTERFACE,'Gateway eth0');
    const existing=(await rpc({oneofKind:'network',network:m.Network.create({kind:m.AssetKind.IP_ADDRESS,name:'Existing VRRP',cidr:'192.0.2.81/32',addressSpace:space,link,sharing:m.SharedUse.VRRP,vrid:41,groupName:'Existing gateway group'})})).mutation.targets[0];
    const group=(await rpc({oneofKind:'read',read:m.Read.create({target:existing})})).detail.group;
    for (const [index,mode] of ['existing','new','group'].entries()) {
      const address=`192.0.2.${81+index}`;
      const fact=(await rpc({oneofKind:'address',address:m.Address.create({address,meaning:m.AddressMeaning.CONFIGURED})})).mutation.targets[0];
      await page.setViewportSize({width:index===0?1440:390,height:900});
      await page.goto(`${server.url}/#/?asset=${fact.assetId}&incarnation=${fact.incarnation}`);
      await page.getByRole('button',{name:'Resolve address in broader scope',exact:true}).click();
      const dialog=page.getByRole('dialog',{name:'Resolve address in broader scope',exact:true});
      await dialog.getByRole('button',{name:'Choose allocation context',exact:true}).click();
      if (mode==='existing') {
        await dialog.getByLabel('Canonical allocation',{exact:true}).selectOption(`${existing.assetId}/${existing.incarnation}`);
        await dialog.getByRole('button',{name:'Review address mapping',exact:true}).click();
      } else {
        await dialog.getByRole('button',{name:'Prepare new allocation',exact:true}).click();
        const allocation=page.getByRole('dialog',{name:'Allocate network resource',exact:true});
        // Reconciliation maps this exact address; next-available allocation cannot satisfy it.
        await expect(allocation.getByRole('checkbox', { name: 'Atomically allocate next available host in the supplied prefix', exact: true })).toHaveCount(0);
        await allocation.getByLabel('Name',{exact:true}).fill(`Prepared ${mode}`);
        await allocation.getByLabel('Address / prefix',{exact:true}).fill(`${address}/32`);
        await allocation.getByLabel('Address space',{exact:true}).selectOption(space.assetId);
        await allocation.getByLabel('Address use',{exact:true}).selectOption(String(m.SharedUse.VRRP));
        if (mode==='group') {
          await allocation.getByLabel('Group selection',{exact:true}).selectOption('existing');
          await allocation.getByLabel('Shared address group',{exact:true}).selectOption(group.target.assetId);
        } else {
          await allocation.getByLabel('L2 domain',{exact:true}).selectOption(link.assetId);
          await allocation.getByLabel('VRID',{exact:true}).fill('42');
          await allocation.getByLabel('New group name',{exact:true}).fill('New gateway group');
        }
        await allocation.getByRole('button',{name:'Review allocation',exact:true}).click();
        await expect(allocation).toBeHidden();
      }
      await dialog.getByLabel('Intended assignment',{exact:true}).selectOption(`${iface.assetId}/${iface.incarnation}`);
      await expect(dialog.getByLabel('Intended priority',{exact:true})).toBeVisible();
      await dialog.getByLabel('Intended priority',{exact:true}).fill(String(150+index));
      await dialog.getByRole('button',{name:'Apply reviewed mapping',exact:true}).click();
      await expect(dialog).toBeHidden();
      const item=(await rpc({oneofKind:'read',read:m.Read.create({target:fact})})).detail.item;
      const binding=item.fields.find(f=>f.name==='binding').value.text.split('/');
      const detail=(await rpc({oneofKind:'read',read:m.Read.create({target:{assetId:binding[0],incarnation:binding[1]}})})).detail;
      expect(detail.participants).toHaveLength(1);
      expect(detail.participants[0].interface).toEqual(iface);
      expect(detail.participants[0].priority).toBe(150+index);
    }
  } finally { await server.stop(); await rm(dir,{recursive:true,force:true}); }
});

test('address mapping reviews a new allocation and commits its explicit assignment together', async ({ page }) => {
  // Preparing a new canonical address must not allocate before the final mapping review.
  const dir = await mkdtemp(join(tmpdir(), 'omloom-binding-ui-')); let server = await host(dir);
  try {
    await page.goto(server.url); await create(page,'Real namespace','ADDRESS_SPACE');
    const iface = await create(page,'Guest eth0','INTERFACE');
    await page.getByRole('button',{name:'+ Create resource',exact:true}).click();
    let dialog = page.getByRole('dialog');
    await dialog.getByLabel('Resource kind',{exact:true}).selectOption({label:'address reference'});
    await dialog.getByLabel('IP address',{exact:true}).fill('192.0.2.80');
    await dialog.getByRole('button',{name:'More known details',exact:true}).click();
    await dialog.getByLabel('Address meaning change',{exact:true}).selectOption('set');
    await dialog.getByLabel('Address meaning',{exact:true}).selectOption({label:'configured'});
    await dialog.getByRole('button',{name:'Save known address',exact:true}).click(); await expect(dialog).toBeHidden();
    await page.getByRole('button',{name:'Resolve address in broader scope',exact:true}).click();
    dialog = page.getByRole('dialog',{name:'Resolve address in broader scope',exact:true});
    await dialog.getByRole('button',{name:'Choose allocation context',exact:true}).click();
    await dialog.getByRole('button',{name:'Prepare new allocation',exact:true}).click();
    const allocation = page.getByRole('dialog',{name:'Allocate network resource',exact:true});
    await allocation.getByLabel('Name',{exact:true}).fill('Guest allocation');
    await allocation.getByLabel('Address / prefix',{exact:true}).fill('192.0.2.80/32');
    await allocation.getByLabel('Address space',{exact:true}).selectOption({label:await allocation.getByLabel('Address space',{exact:true}).locator('option').filter({hasText:'Real namespace'}).textContent()});
    await allocation.getByRole('button',{name:'Review allocation',exact:true}).click(); await expect(allocation).toBeHidden();
    await expect(dialog).toContainText('commit together');
    await dialog.getByLabel('Intended assignment',{exact:true}).selectOption(`${iface.asset_id}/${iface.incarnation}`);
    await dialog.getByRole('button',{name:'Apply reviewed mapping',exact:true}).click(); await expect(dialog).toBeHidden();
    await expect(page.getByRole('region',{name:'Resource details'}).locator('dt').filter({hasText:/^binding$/})).toHaveCount(1);
    const hash = new URL(page.url()).hash; await server.stop(); server = await host(dir); await page.goto(server.url+hash);
    await expect(page.getByRole('region',{name:'Resource details'})).toContainText('192.0.2.80');
    await page.goto(server.url); await page.getByRole('button',{name:'Guest allocation',exact:true}).click();
    await expect(page.getByRole('button',{name:`Interface ${iface.asset_id.slice(-8)}`,exact:true})).toBeVisible();
  } finally { await server.stop(); await rm(dir,{recursive:true,force:true}); }
});

test('prefix context allocates and retrieves children without reselecting its namespace', async ({ page }) => {
  // The containing prefix is an explicit allocation context, not an inherited VRF.
  const dir = await mkdtemp(join(tmpdir(), 'omloom-prefix-ui-')); const server = await host(dir);
  try {
    await page.goto(server.url); await create(page,'Namespace under review','ADDRESS_SPACE');
    await page.getByRole('navigation',{name:'Views'}).getByRole('button',{name:'Network',exact:true}).click();
    await page.getByRole('button',{name:'Allocate IP / prefix',exact:true}).click();
    const dialog = page.getByRole('dialog');
    await expect(dialog).toContainText('Within Namespace under review');
    await expect(dialog.getByLabel('Address space',{exact:true})).toHaveCount(0);
    await dialog.getByLabel('Kind',{exact:true}).selectOption({label:'Prefix'});
    await dialog.getByLabel('Name',{exact:true}).fill('Guest subnet');
    await dialog.getByLabel('Address / prefix',{exact:true}).fill('2001:db8:1::/64');
    await dialog.getByRole('button',{name:'Allocate',exact:true}).click(); await expect(dialog).toBeHidden();
    await page.getByRole('button',{name:'Guest subnet',exact:true}).click();
    await page.getByRole('button',{name:'Allocate IP / prefix',exact:true}).click();
    await expect(dialog).toContainText('Within Guest subnet');
    await dialog.getByLabel('Name',{exact:true}).fill('Guest address');
    await dialog.getByLabel('Address / prefix',{exact:true}).fill('2001:db8:1::10/128');
    await dialog.getByRole('button',{name:'Allocate',exact:true}).click(); await expect(dialog).toBeHidden();
    await page.getByLabel('Search inventory').fill('2001:db8:1::10');
    await page.getByRole('button',{name:'Search',exact:true}).click();
    await expect(page.getByRole('button',{name:'Guest address',exact:true})).toBeVisible();
    await page.reload(); await expect(page.getByRole('button',{name:'Guest address',exact:true})).toBeVisible();
  } finally { await server.stop(); await rm(dir,{recursive:true,force:true}); }
});

test('scoped allocation edits review their enclosing network references', async ({ page }) => {
  // A prefix scope excludes its namespace; an edit must explicitly review both.
  const dir = await mkdtemp(join(tmpdir(), 'omloom-network-review-'));
  const server = await host(dir);
  try {
    const { m, rpc } = await workspaceClient(server.url);
    const space = (await rpc({ oneofKind: 'write', write: m.Write.create({ kind: m.AssetKind.ADDRESS_SPACE, changes: [{ name: 'name', value: { oneofKind: 'text', text: 'Reviewed namespace' } }] }) })).mutation.targets[0];
    const prefix = (await rpc({ oneofKind: 'network', network: m.Network.create({ kind: m.AssetKind.PREFIX, name: 'Reviewed subnet', cidr: '192.0.2.0/24', addressSpace: space, sharing: m.SharedUse.EXCLUSIVE }) })).mutation.targets[0];
    const target = (await rpc({ oneofKind: 'network', network: m.Network.create({ kind: m.AssetKind.IP_ADDRESS, name: 'Reviewed IP', cidr: '192.0.2.10/32', addressSpace: space, sharing: m.SharedUse.EXCLUSIVE }) })).mutation.targets[0];
    for (const width of [1440, 390]) {
      await page.setViewportSize({ width, height: 900 });
      await page.goto(`${server.url}/#/?view=1&scope=${prefix.assetId}&scopeInc=${prefix.incarnation}&asset=${target.assetId}&incarnation=${target.incarnation}`);
      await page.getByRole('button', { name: 'Review network settings', exact: true }).click();
      const dialog = page.getByRole('dialog');
      await dialog.getByLabel('Name', { exact: true }).fill(`Reviewed IP ${width}`);
      // The initial submission must not mutate in an incomplete context.
      await dialog.getByRole('button', { name: 'Apply reviewed settings', exact: true }).click();
      await expect(dialog).toBeVisible();
      // Delay the real prepare reply while the user edits; it must not certify the new values.
      let release, received;
      const pending = new Promise(resolve => { received = resolve; });
      const gate = new Promise(resolve => { release = resolve; });
      const delayed = async route => {
        const request = m.Request.fromBinary(route.request().postDataBuffer().subarray(5));
        if (request.command.oneofKind !== 'prepare') return route.continue();
        const response = await route.fetch(); received(); await gate;
        await route.fulfill({ response });
      };
      await page.route('**/omloom.workspace.v1.Workspace/Call', delayed);
      await dialog.getByRole('button', { name: 'Review broader network context', exact: true }).click();
      await pending;
      await dialog.getByLabel('Name', { exact: true }).fill(`Reviewed IP changed ${width}`);
      const replied = page.waitForResponse(response => response.url().endsWith('/omloom.workspace.v1.Workspace/Call'));
      release();
      await replied;
      await page.evaluate(() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve))));
      await expect(dialog.getByText('Context reviewed. Changing the settings requires a new review.', { exact: true })).toBeHidden({ timeout: 1000 });
      await page.unroute('**/omloom.workspace.v1.Workspace/Call', delayed);
      await dialog.getByLabel('Name', { exact: true }).fill(`Reviewed IP ${width}`);
      await dialog.getByRole('button', { name: 'Review broader network context', exact: true }).click();
      await expect(dialog.getByRole('region', { name: 'Reviewed network context' })).toContainText('Reviewed namespace');
      await dialog.getByRole('button', { name: 'Apply reviewed settings', exact: true }).click();
      await expect(dialog).toBeHidden();
      const item = (await rpc({ oneofKind: 'read', read: m.Read.create({ target }) })).detail.item;
      expect(item.fields.find(f => f.name === 'name').value.text).toBe(`Reviewed IP ${width}`);
      expect(item.fields.find(f => f.name === 'address_space').value.text).toBe(`${space.assetId}/${space.incarnation}`);
      expect(new URL(page.url()).hash).toContain(`scope=${prefix.assetId}`);
    }
  } finally { await server.stop(); await rm(dir, { recursive: true, force: true }); }
});

test('network replacement cannot clear undisclosed references', async ({ page }) => {
  // A host field projection must not turn hidden VRF/L2 settings into clears.
  const dir = await mkdtemp(join(tmpdir(), 'omloom-network-projection-'));
  let server = await host(dir);
  try {
    const { m, rpc } = await workspaceClient(server.url);
    const object = async (kind, name) => (await rpc({ oneofKind: 'write', write: m.Write.create({ kind, changes: [{ name: 'name', value: { oneofKind: 'text', text: name } }] }) })).mutation.targets[0];
    const space = await object(m.AssetKind.ADDRESS_SPACE, 'Visible namespace');
    const routing = await object(m.AssetKind.VRF, 'Private routing');
    const link = await object(m.AssetKind.L2_DOMAIN, 'Private link');
    const target = (await rpc({ oneofKind: 'network', network: m.Network.create({ kind: m.AssetKind.IP_ADDRESS, name: 'Protected allocation', cidr: '192.0.2.20/32', addressSpace: space, link, routing, sharing: m.SharedUse.EXCLUSIVE }) })).mutation.targets[0];
    await server.stop();
    const grants = join(dir, 'grants.json');
    await writeFile(grants, JSON.stringify([{ selection: { scope: 'all' }, read: { mode: 'selected', fields: ['name', 'cidr', 'address_space', 'sharing'] }, write: { mode: 'all' }, writable: true }]));
    server = await host(dir, 'operator', grants);
    for (const width of [1440, 390]) {
      await page.setViewportSize({ width, height: 900 });
      await page.goto(`${server.url}/#/?view=1&asset=${target.assetId}&incarnation=${target.incarnation}`);
      await expect(page.getByRole('region', { name: 'Resource details' })).not.toContainText('Private routing');
      await page.getByRole('button', { name: 'Review network settings', exact: true }).click();
      const dialog = page.getByRole('dialog');
      await dialog.getByLabel('Name', { exact: true }).fill('Must not replace');
      await dialog.getByRole('button', { name: 'Apply reviewed settings', exact: true }).click();
      await expect(dialog.getByRole('alert')).toContainText('PERMISSION_DENIED');
      await dialog.getByRole('button', { name: 'Cancel', exact: true }).click();
      await expect(dialog).toBeHidden();
    }
    await page.goto('about:blank');
    await server.stop();
    server = await host(dir);
    const client = await workspaceClient(server.url);
    const item = (await client.rpc({ oneofKind: 'read', read: m.Read.create({ target }) })).detail.item;
    const field = name => item.fields.find(f => f.name === name).value.text;
    expect(field('name')).toBe('Protected allocation');
    expect(field('routing_context')).toBe(`${routing.assetId}/${routing.incarnation}`);
    expect(field('link_scope')).toBe(`${link.assetId}/${link.incarnation}`);
  } finally { await server.stop(); await rm(dir, { recursive: true, force: true }); }
});

test('workspace creation starts with the relevant resource type', async ({ page }) => {
  // Views share inventory but should not make every creation start as a physical device.
  const dir = await mkdtemp(join(tmpdir(), 'omloom-context-ui-'));
  const server = await host(dir);
  try {
    await page.goto(server.url);
    for (const [view, kind] of [['Virtualization', 'virtual machine'], ['Software & services', 'service'], ['Hardware', 'device']]) {
      await page.getByRole('navigation', { name: 'Views' }).getByRole('button', { name: view, exact: true }).click();
      await page.getByRole('button', { name: '+ Create resource', exact: true }).click();
      await expect(page.getByRole('dialog').getByLabel('Resource kind', { exact: true }).locator('option:checked')).toHaveText(kind);
      await page.getByRole('dialog').getByRole('button', { name: 'Cancel', exact: true }).click();
    }
    await page.getByRole('navigation', { name: 'Views' }).getByRole('button', { name: 'Network', exact: true }).click();
    await page.getByRole('button', { name: '+ Create resource', exact: true }).click();
    await expect(page.getByRole('dialog').getByRole('heading', { name: 'Record known address', exact: true })).toBeVisible();
    await page.getByRole('dialog').getByLabel('IP address', { exact: true }).fill('192.0.2.17');
    await expect(page.getByRole('dialog').getByLabel('Address space', { exact: true })).toHaveCount(0);
    await expect(page.getByRole('dialog').getByLabel('Routing context', { exact: true })).toHaveCount(0);
    await page.getByRole('dialog').getByRole('button', { name: 'Save known address', exact: true }).click();
    await expect(page.getByRole('dialog')).toBeHidden();
    await expect(page.getByRole('region', { name: 'Resource details' })).toContainText('192.0.2.17');
    await page.getByRole('button', { name: 'Close resource details', exact: true }).click();
    await page.getByRole('button', { name: 'Allocate IP / prefix', exact: true }).click();
    await expect(page.getByRole('dialog').getByRole('heading', { name: 'Allocate network resource', exact: true })).toBeVisible();
    await expect(page.getByRole('dialog').getByRole('checkbox', { name: 'Atomically allocate next available host in the supplied prefix', exact: true })).toBeVisible();
    await page.getByRole('dialog').getByLabel('Name', { exact: true }).fill('Pending address');
    await page.getByRole('dialog').getByRole('button', { name: 'Create address space', exact: true }).click();
    await page.getByRole('dialog', { name: 'Create resource', exact: true }).getByLabel('Name', { exact: true }).fill('Local network scope');
    await page.getByRole('dialog', { name: 'Create resource', exact: true }).getByRole('button', { name: 'Save resource', exact: true }).click();
    await expect(page.getByRole('dialog').getByRole('heading', { name: 'Allocate network resource', exact: true })).toBeVisible();
    await expect(page.getByRole('dialog').getByLabel('Address space', { exact: true }).locator('option').filter({ hasText: 'Local network scope' })).toHaveCount(1);
    await expect(page.getByRole('dialog').getByLabel('Name', { exact: true })).toHaveValue('Pending address');
  } finally { await server.stop(); await rm(dir, { recursive: true, force: true }); }
});

test('scope transitions discard enclosing pagination and preserve back navigation', async ({ page }) => {
  // A cursor belongs to its original selector, including when a detail switches to a new root.
  test.setTimeout(60_000);
  const { buildSync } = await import('esbuild');
  const { GrpcWebFetchTransport } = await import('@protobuf-ts/grpcweb-transport');
  const built = buildSync({ stdin: { contents: 'export * from "./web/src/generated/omloom/v1/workspace"; export {AssetKind} from "./web/src/generated/omloom/v1/contracts";', resolveDir: process.cwd() }, bundle: true, write: false, format: 'esm', platform: 'node' });
  const m = await import('data:text/javascript;base64,' + Buffer.from(built.outputFiles[0].contents).toString('base64'));
  const dir = await mkdtemp(join(tmpdir(), 'omloom-scope-page-'));
  const server = await host(dir);
  try {
    const transport = new GrpcWebFetchTransport({ baseUrl: server.url, format: 'binary' });
    async function rpc(command, focus) {
      const q = await transport.unary(m.Workspace.methods[0], m.Request.create({ command: { oneofKind: 'query', query: m.Query.create({ limit: 1 }) } }), transport.mergeOptions());
      return (await transport.unary(m.Workspace.methods[0], m.Request.create({ command, working: { focus, expected: q.response.result.page.revision } }), transport.mergeOptions())).response.result;
    }
    let chassis;
    for (let i = 0; i < 26; i++) chassis = (await rpc({ oneofKind: 'write', write: m.Write.create({ kind: m.AssetKind.DEVICE, changes: [{ name: 'name', value: { oneofKind: 'text', text: `Chassis ${String(i).padStart(2, '0')}` } }] }) })).mutation.targets[0];
    const port = (await rpc({ oneofKind: 'write', write: m.Write.create({ kind: m.AssetKind.PORT, parent: chassis, changes: [{ name: 'name', value: { oneofKind: 'text', text: 'Scoped port' } }] }) }, chassis)).mutation.targets[0];
    const params = () => new URLSearchParams(new URL(page.url()).hash.split('?')[1]);
    for (const width of [1440, 390]) {
      await page.setViewportSize({ width, height: 900 });
      await page.goto(`${server.url}/#/?view=2&columns=name&sort=name`);
      await page.getByRole('button', { name: 'Next page →', exact: true }).click();
      await expect(page.getByRole('button', { name: 'Chassis 25', exact: true })).toBeVisible();
      const cursor = params().get('cursor');
      expect(cursor).toBeTruthy();
      await page.getByRole('button', { name: 'Chassis 25', exact: true }).click();
      const detail = page.getByRole('region', { name: 'Resource details', exact: true });
      await detail.getByRole('button', { name: 'Use as scope', exact: true }).click();
      await expect.poll(() => params().get('scope')).toBe(chassis.assetId);
      expect(params().get('cursor')).toBeNull();
      await expect(page.getByRole('alert')).toHaveCount(0);
      await expect(page.getByRole('button', { name: 'Scoped port', exact: true })).toBeVisible();
      await page.goBack();
      await detail.getByRole('button', { name: 'Close resource details', exact: true }).click();
      await expect(page.getByRole('button', { name: 'Chassis 25', exact: true })).toBeVisible();
      expect(params().get('cursor')).toBe(cursor);
      if (width === 1440) {
        await page.getByRole('button', { name: 'Chassis 25', exact: true }).click();
        await page.getByRole('checkbox', { name: 'Select Scoped port', exact: true }).check();
        await page.getByRole('button', { name: 'Use selected as scope', exact: true }).click();
        await expect.poll(() => params().get('scope')).toBe(port.assetId);
        expect(params().get('cursor')).toBeNull();
        await expect(page.getByRole('alert')).toHaveCount(0);
      }
    }
  } finally { await server.stop(); await rm(dir, { recursive: true, force: true }); }
});

test('dense containers paginate and restore the enclosing view on desktop and phone', async ({page}) => {
  // Fixture creation is separate from the bounded user actions; each UI action retains its normal timeout.
  test.setTimeout(60_000);
  const {buildSync}=await import('esbuild');
  const {GrpcWebFetchTransport}=await import('@protobuf-ts/grpcweb-transport');
  const built=buildSync({stdin:{contents:'export * from "./web/src/generated/omloom/v1/workspace"; export {AssetKind} from "./web/src/generated/omloom/v1/contracts";',resolveDir:process.cwd()},bundle:true,write:false,format:'esm',platform:'node'});
  const m=await import('data:text/javascript;base64,'+Buffer.from(built.outputFiles[0].contents).toString('base64'));
  const dir=await mkdtemp(join(tmpdir(),'omloom-dense-ui-'));const server=await host(dir);
  try {
    const transport=new GrpcWebFetchTransport({baseUrl:server.url,format:'binary'});
    async function rpc(command,focus){
      const q=await transport.unary(m.Workspace.methods[0],m.Request.create({command:{oneofKind:'query',query:m.Query.create({limit:1})}}),transport.mergeOptions());
      return (await transport.unary(m.Workspace.methods[0],m.Request.create({command,working:{roots:[],focus,expected:q.response.result.page.revision}}),transport.mergeOptions())).response.result;
    }
    async function object(kind,name,parent){return (await rpc({oneofKind:'write',write:m.Write.create({kind,parent,changes:[{name:'name',value:{oneofKind:'text',text:name}}]})},parent)).mutation.targets[0];}
    const device=await object(m.AssetKind.DEVICE,'Dense chassis');
    for(let i=0;i<129;i++) await object(m.AssetKind.PORT,`Port ${String(i).padStart(3,'0')}`,device);
    const root=`${server.url}/#/?view=2&search=Dense&sort=name&desc=1&columns=name,serial`;
    for(const width of [1440,390]){
      await page.setViewportSize({width,height:844});await page.goto(root);
      await page.getByRole('checkbox',{name:'Select Dense chassis',exact:true}).check();
      await page.getByRole('button',{name:'Dense chassis',exact:true}).click();
      const details=page.getByRole('region',{name:'Resource details',exact:true});
      await expect(details.getByRole('heading',{name:'Dense chassis',exact:true})).toBeVisible();
      await expect(page.getByRole('alert')).toHaveCount(0);
      await expect(details.getByRole('button',{name:'Next relationships',exact:true})).toBeVisible();
      const firstRelations=await details.locator(':scope > ul').innerText();
      await details.getByRole('button',{name:'Next relationships',exact:true}).click();
      await expect.poll(()=>details.locator(':scope > ul').innerText()).not.toBe(firstRelations);
      if(width<700) await page.getByLabel('Resource perspective',{exact:true}).selectOption('4');
      else await page.getByRole('navigation',{name:'Views',exact:true}).getByRole('button',{name:'Software & services',exact:true}).click();
      await details.getByRole('button',{name:'Close resource details',exact:true}).click();
      await expect(page.getByRole('heading',{name:'Hardware',exact:true})).toBeVisible();
      await expect(page.getByLabel('Search inventory',{exact:true})).toHaveValue('Dense');
      await expect(page.getByRole('checkbox',{name:'Select Dense chassis',exact:true})).toBeChecked();
      await page.getByRole('button',{name:'Dense chassis',exact:true}).click();
      await page.reload();
      await details.getByRole('button',{name:'Close resource details',exact:true}).click();
      await expect(page.getByRole('heading',{name:'Hardware',exact:true})).toBeVisible();
      await expect(page.getByLabel('Search inventory',{exact:true})).toHaveValue('Dense');
      expect(new URL(page.url()).hash).toContain('desc=1');
      expect(new URL(page.url()).hash).toContain('columns=name,serial');
      expect(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth)).toBe(true);
      await page.screenshot({path:`test-results/dense-container-${width}.png`,fullPage:true});
    }
  } finally {await server.stop();await rm(dir,{recursive:true,force:true});}
});

test('phone inventory opens a dedicated detail and returns to the same search', async ({ page }) => {
  // A phone should not require scrolling past the desktop table to inspect or edit its selected asset.
  const dir = await mkdtemp(join(tmpdir(), 'omloom-phone-ui-'));
  const server = await host(dir);
  try {
    await page.goto(server.url);
    await create(page, 'Field router');
    for (const name of ['Field node A', 'Field node B', 'Field node C', 'Field node D']) await create(page, name);
    await page.getByRole('button', { name: 'Close resource details', exact: true }).click();
    await page.setViewportSize({ width: 390, height: 844 });
    await expect(page.getByRole('button', { name: 'Import CSV', exact: true })).toBeHidden();
    await page.getByRole('button', { name: 'More', exact: true }).click();
    await expect(page.getByRole('button', { name: 'Import CSV', exact: true })).toBeVisible();
    await expect(page.getByRole('button', { name: 'Refresh inventory', exact: true })).toBeVisible();
    await page.keyboard.press('Escape');
    await expect(page.getByRole('button', { name: 'More', exact: true })).toHaveAttribute('aria-expanded', 'false');
    expect((await page.locator('tbody tr').first().boundingBox()).y).toBeLessThan(560);
    await page.getByLabel('Search inventory').fill('Field');
    await page.getByRole('button', { name: 'Search', exact: true }).click();
    await page.getByRole('checkbox', { name: 'Select Field router', exact: true }).check();
    await page.getByRole('button', { name: 'Field router', exact: true }).scrollIntoViewIfNeeded();
    const listPosition = await page.evaluate(() => scrollY);
    expect(listPosition).toBeGreaterThan(0);
    await page.getByRole('button', { name: 'Field router', exact: true }).click();
    await expect(page.getByRole('region', { name: 'Inventory', exact: true })).toBeHidden();
    await expect(page.getByRole('region', { name: 'Resource details' })).toBeVisible();
    await page.screenshot({ path: 'test-results/phone-resource-detail.png', fullPage: true });
    await page.getByRole('button', { name: 'Edit resource', exact: true }).click();
    const dialog = page.getByRole('dialog');
    await dialog.getByLabel('Description change', { exact: true }).selectOption('set');
    await dialog.getByLabel('Description', { exact: true }).fill('Checked on site');
    await page.screenshot({ path: 'test-results/phone-edit-resource.png', fullPage: true });
    await dialog.getByRole('button', { name: 'Save resource', exact: true }).click();
    await expect(dialog).toBeHidden();
    await expect(page.getByRole('region', { name: 'Resource details' })).toContainText('Checked on site');
    // Changing representation inside a detail must not replace its return destination.
    await page.getByLabel('Resource perspective', { exact: true }).selectOption('4');
    await page.getByRole('button', { name: 'Back to inventory', exact: true }).click();
    await expect(page.getByRole('heading', { name: 'Overview', exact: true })).toBeVisible();
    await expect(page.getByLabel('Search inventory')).toHaveValue('Field');
    await expect.poll(() => page.evaluate(() => scrollY)).toBe(listPosition);
    await expect(page.getByRole('checkbox', { name: 'Select Field router', exact: true })).toBeChecked();
    await expect(page.getByRole('button', { name: 'Field router', exact: true })).toBeVisible();
    await page.getByLabel('Sort resources', { exact: true }).selectOption('name:desc');
    await page.getByLabel('Select phone page', { exact: true }).check();
    await expect(page.getByText('5 selected on this page', { exact: false })).toBeVisible();
    await page.getByRole('button', { name: 'Clear selection', exact: true }).click();
    await page.screenshot({ path: 'test-results/phone-inventory-list.png', fullPage: true });
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    await page.reload();
    await expect(page.getByLabel('Search inventory')).toHaveValue('Field');
  } finally { await server.stop(); await rm(dir, { recursive: true, force: true }); }
});

test('known address display names work as typed name filters', async ({ page }) => {
  // The rendered label and typed name filter must agree without inventing an intent field.
  const dir = await mkdtemp(join(tmpdir(), 'omloom-address-name-'));
  const server = await host(dir);
  try {
    const { m, rpc } = await workspaceClient(server.url);
    for (const address of ['192.0.2.10', '2001:db8::10']) await rpc({ oneofKind: 'address', address: m.Address.create({ address }) });
    for (const width of [1440, 390]) {
      await page.setViewportSize({ width, height: 900 });
      for (const address of ['192.0.2.10', '2001:db8::10']) {
        await page.goto(`${server.url}/#/?view=1&kind=${m.AssetKind.ADDRESS_REFERENCE}`);
        await page.getByRole('button', { name: 'Add filter', exact: true }).click();
        const dialog = page.getByRole('dialog');
        await dialog.getByLabel('Field', { exact: true }).selectOption('name');
        await dialog.getByLabel('Equals', { exact: true }).fill(address);
        await dialog.getByRole('button', { name: 'Apply filter', exact: true }).click();
        await expect(dialog).toBeHidden();
        await expect(page.getByRole('button', { name: address, exact: true })).toBeVisible();
        await page.reload();
        await expect(page.getByRole('button', { name: address, exact: true })).toBeVisible();
        await page.getByRole('button', { name: address, exact: true }).click();
        await expect(page.getByRole('region', { name: 'Resource details' }).getByRole('heading', { name: address, exact: true })).toBeVisible();
      }
    }
  } finally { await server.stop(); await rm(dir, { recursive: true, force: true }); }
});

test('scoped participants explicitly review allocation group and interface', async ({ page }) => {
  // Namespace membership is not interface containment; assignment needs a reviewed peer context.
  const dir = await mkdtemp(join(tmpdir(), 'omloom-member-context-'));
  const server = await host(dir);
  try {
    const { m, rpc } = await workspaceClient(server.url);
    const object = async (kind, name) => (await rpc({ oneofKind: 'write', write: m.Write.create({ kind, changes: [{ name: 'name', value: { oneofKind: 'text', text: name } }] }) })).mutation.targets[0];
    const space = await object(m.AssetKind.ADDRESS_SPACE, 'Participant namespace');
    const prefix = (await rpc({ oneofKind: 'network', network: m.Network.create({ kind: m.AssetKind.PREFIX, name: 'Participant subnet', cidr: '192.0.2.0/24', addressSpace: space, sharing: m.SharedUse.EXCLUSIVE }) })).mutation.targets[0];
    const link = await object(m.AssetKind.L2_DOMAIN, 'Participant LAN');
    const iface = await object(m.AssetKind.INTERFACE, 'Reviewed uplink');
    for (const sharing of [m.SharedUse.EXCLUSIVE, m.SharedUse.VRRP]) {
      const target = (await rpc({ oneofKind: 'network', network: m.Network.create({ kind: m.AssetKind.IP_ADDRESS, name: `Participant allocation ${sharing}`, cidr: `192.0.2.${10 + sharing}/32`, addressSpace: space, sharing, link: sharing === m.SharedUse.VRRP ? link : undefined, vrid: sharing === m.SharedUse.VRRP ? 7 : undefined, groupName: sharing === m.SharedUse.VRRP ? 'Reviewed routers' : '' }) })).mutation.targets[0];
      for (const [index, root] of [space, prefix].entries()) {
        await page.setViewportSize({ width: index ? 390 : 1440, height: 900 });
        await page.goto(`${server.url}/#/?view=1&scope=${root.assetId}&scopeInc=${root.incarnation}&asset=${target.assetId}&incarnation=${target.incarnation}`);
        await expect(page.getByRole('region', { name: 'Resource details' }).getByRole('heading', { name: `Participant allocation ${sharing}`, exact: true })).toBeVisible();
        await page.getByRole('button', { name: 'Assign participant', exact: true }).click();
        const dialog = page.getByRole('dialog');
        await expect(dialog.getByLabel('Interface', { exact: true }).locator('option')).toHaveCount(1);
        await dialog.getByRole('button', { name: 'Choose across permitted inventory', exact: true }).click({ timeout: 3000 });
        await dialog.getByLabel('Interface', { exact: true }).selectOption(`${iface.assetId}/${iface.incarnation}`);
        if (sharing === m.SharedUse.VRRP) await dialog.getByLabel('Intended priority', { exact: true }).fill('150');
        await dialog.getByRole('button', { name: 'Review participant assignment', exact: true }).click();
        const review = dialog.getByRole('region', { name: 'Reviewed participant context' });
        await expect(review).toContainText('Reviewed uplink');
        if (sharing === m.SharedUse.VRRP) await expect(review).toContainText('Reviewed routers');
        // Changing the endpoint invalidates the reviewed tuple, even if reselected.
        await dialog.getByLabel('Interface', { exact: true }).selectOption('');
        await expect(review).toBeHidden();
        await dialog.getByLabel('Interface', { exact: true }).selectOption(`${iface.assetId}/${iface.incarnation}`);
        await dialog.getByRole('button', { name: 'Review participant assignment', exact: true }).click();
        await expect(review).toBeVisible();
        await dialog.getByRole('button', { name: 'Assign participant', exact: true }).click();
        await expect(dialog).toBeHidden();
        const detail = (await rpc({ oneofKind: 'read', read: m.Read.create({ target }) })).detail;
        expect(detail.participants.map(p => p.interface)).toEqual([iface]);
        expect(new URL(page.url()).hash).toContain(`scope=${root.assetId}`);
        // Undo uses the same explicit peer review, without widening the browsing scope.
        await page.getByRole('button', { name: 'Remove participant', exact: true }).click();
        await dialog.getByRole('button', { name: 'Review participant removal', exact: true }).click({ timeout: 3000 });
        await expect(dialog.getByRole('region', { name: 'Reviewed participant context' })).toContainText('Reviewed uplink');
        await dialog.getByRole('button', { name: 'Remove participant', exact: true }).click();
        await expect(dialog).toBeHidden();
        expect((await rpc({ oneofKind: 'read', read: m.Read.create({ target }) })).detail.participants).toEqual([]);
        expect(new URL(page.url()).hash).toContain(`scope=${root.assetId}`);
      }
    }
  } finally { await server.stop(); await rm(dir, { recursive: true, force: true }); }
});

test('shared gateway addresses use one durable group and intended participant set', async ({ page }) => {
  // One group spans two IPs; the UI must send its revision when editing through either address.
  const dir = await mkdtemp(join(tmpdir(), 'omloom-ipam-ui-'));
  let server = await host(dir);
  try {
    await page.goto(server.url);
    await create(page, 'Tenant addresses', 'ADDRESS_SPACE');
    await create(page, 'Edge LAN', 'L2_DOMAIN');
    await create(page, 'Router uplink', 'INTERFACE');
    await page.getByRole('button', { name: 'Close resource details', exact: true }).click();
    await page.getByRole('navigation', { name: 'Views' }).getByRole('button', { name: 'Network', exact: false }).click();
    for (const [index, name] of ['Gateway A', 'Gateway B'].entries()) {
      await page.getByRole('button', { name: 'Allocate IP / prefix', exact: true }).click();
      const dialog = page.getByRole('dialog');
      await dialog.getByLabel('Name', { exact: true }).fill(name);
      await dialog.getByLabel('Address / prefix', { exact: true }).fill(`192.0.2.${index + 1}/24`);
      await dialog.getByLabel('Address space', { exact: true }).selectOption({ label: await dialog.getByLabel('Address space', { exact: true }).locator('option').filter({ hasText: 'Tenant addresses' }).textContent() });
      await dialog.getByLabel('Address use', { exact: true }).selectOption({ label: 'Explicit VRRP group' });
      if (index === 0) {
        await dialog.getByLabel('L2 domain', { exact: true }).selectOption({ label: await dialog.getByLabel('L2 domain', { exact: true }).locator('option').filter({ hasText: 'Edge LAN' }).textContent() });
        await dialog.getByLabel('New group name', { exact: true }).fill('Edge routers');
        await dialog.getByLabel('VRID', { exact: true }).fill('7');
      } else {
        await dialog.getByLabel('Group selection', { exact: true }).selectOption('existing');
        await dialog.getByLabel('Shared address group', { exact: true }).selectOption({ label: await dialog.getByLabel('Shared address group', { exact: true }).locator('option').filter({ hasText: 'Edge routers' }).textContent() });
      }
      await dialog.getByRole('button', { name: 'Allocate', exact: true }).click();
      await expect(dialog).toBeHidden();
    }
    await page.getByRole('button', { name: 'Gateway A', exact: true }).click();
    await page.getByRole('button', { name: 'Assign participant', exact: true }).click();
    const dialog = page.getByRole('dialog');
    await dialog.getByLabel('Interface', { exact: true }).selectOption({ label: await dialog.getByLabel('Interface', { exact: true }).locator('option').filter({ hasText: 'Router uplink' }).textContent() });
    await dialog.getByLabel('Intended priority', { exact: true }).fill('255');
    await dialog.getByRole('button', { name: 'Review participant assignment', exact: true }).click();
    await dialog.getByRole('button', { name: 'Assign participant', exact: true }).click();
    await expect(dialog).toBeHidden();
    await page.getByRole('button', { name: 'Close resource details', exact: true }).click();
    await page.getByRole('button', { name: 'Gateway B', exact: true }).click();
    await expect(page.getByRole('region', { name: 'Resource details' })).toContainText('Intended priority 255');
    const hash = new URL(page.url()).hash;
    await server.stop(); server = await host(dir);
    await page.goto(server.url + hash);
    await expect(page.getByRole('region', { name: 'Resource details' })).toContainText('Intended priority 255');
    await page.getByRole('button', { name: 'Remove participant', exact: true }).click();
    await dialog.getByRole('button', { name: 'Review participant removal', exact: true }).click();
    await expect(dialog.getByRole('region', { name: 'Reviewed participant context' })).toContainText('Router uplink');
    await dialog.getByRole('button', { name: 'Remove participant', exact: true }).click();
    await expect(dialog).toBeHidden();
    await expect(page.getByRole('button', { name: 'Remove participant', exact: true })).toHaveCount(0);
    await page.getByRole('button', { name: 'Close resource details', exact: true }).click();
    await page.getByRole('button', { name: 'Gateway A', exact: true }).click();
    await expect(page.getByRole('region', { name: 'Resource details' })).toContainText('Shared group:');
    await expect(page.getByRole('button', { name: 'Remove participant', exact: true })).toHaveCount(0);
    await page.screenshot({ path: 'test-results/shared-address-group.png', fullPage: true });
  } finally { await server.stop(); await rm(dir, { recursive: true, force: true }); }
});

test('removing a saved audience also removes its editor grant on desktop and phone', async ({ page }) => {
  // Hidden editor grants must not prevent narrowing publication or returning a definition to personal use.
  const { buildSync } = await import('esbuild');
  const { GrpcWebFetchTransport } = await import('@protobuf-ts/grpcweb-transport');
  const built = buildSync({ stdin: { contents: 'export * from "./web/src/generated/omloom/v1/workspace";', resolveDir: process.cwd() }, bundle: true, write: false, format: 'esm', platform: 'node' });
  const m = await import('data:text/javascript;base64,' + Buffer.from(built.outputFiles[0].contents).toString('base64'));
  const dir = await mkdtemp(join(tmpdir(), 'omloom-view-audience-'));
  const server = await host(dir);
  try {
    const transport = new GrpcWebFetchTransport({ baseUrl: server.url, format: 'binary' });
    async function rpc(command) {
      return (await transport.unary(m.Workspace.methods[0], m.Request.create({ command, working: {} }), transport.mergeOptions())).response.result;
    }
    for (const width of [1440, 390]) {
      const view = (await rpc({ oneofKind: 'saveView', saveView: m.SaveView.create({ publish: true, view: m.SavedView.create({ name: `Shared selection ${width}`, query: m.Query.create({ columns: ['name'], limit: 25 }), audiences: ['operations', 'engineering'], editors: ['operations', 'engineering'] }) }) })).views.items[0];
      await page.setViewportSize({ width, height: 900 });
      await page.goto(`${server.url}/#/?saved=${view.id}`);
      await page.reload();
      await expect(page.getByRole('button', { name: 'Reset adjustments', exact: true })).toBeVisible();
      await page.getByRole('button', { name: 'Save view', exact: true }).click();
      const dialog = page.getByRole('dialog');
      await dialog.getByRole('checkbox', { name: 'operations', exact: true }).uncheck();
      await expect(dialog.getByRole('checkbox', { name: 'operations may edit this definition', exact: true })).toHaveCount(0);
      await expect(dialog.getByRole('checkbox', { name: 'engineering may edit this definition', exact: true })).toBeChecked();
      await dialog.getByRole('button', { name: 'Preview publication', exact: true }).click();
      await expect(dialog).toContainText('Publish the name, selected roots, predicates and columns to engineering');
      await dialog.getByRole('button', { name: 'Publish reviewed definition', exact: true }).click();
      await expect(dialog).toBeHidden();
      let saved = (await rpc({ oneofKind: 'views', views: {} })).views.items.find(v => v.id === view.id);
      expect(saved.audiences).toEqual(['engineering']);
      expect(saved.editors).toEqual(['engineering']);
      await page.reload();
      await expect(page.getByRole('button', { name: 'Reset adjustments', exact: true })).toBeVisible();
      await page.getByRole('button', { name: 'Save view', exact: true }).click();
      await dialog.getByRole('checkbox', { name: 'engineering', exact: true }).uncheck();
      await dialog.getByRole('button', { name: 'Save personal view', exact: true }).click();
      await expect(dialog).toBeHidden();
      saved = (await rpc({ oneofKind: 'views', views: {} })).views.items.find(v => v.id === view.id);
      expect(saved.audiences).toEqual([]);
      expect(saved.editors).toEqual([]);
    }
  } finally { await server.stop(); await rm(dir, { recursive: true, force: true }); }
});

test('team publication previews its audience and saved adjustments reset without granting data', async ({ page }) => {
  // The publication preview has no durable effect until confirmed; a different fixture sees only its own data scope.
  const dir = await mkdtemp(join(tmpdir(), 'omloom-view-ui-'));
  let server = await host(dir);
  try {
    await page.goto(server.url);
    await create(page, 'Private hypervisor');
    await create(page, 'Team VM', 'VIRTUAL_MACHINE');
    await page.getByRole('button', { name: 'Close resource details', exact: true }).click();
    await page.getByLabel('Search inventory').fill('Private');
    await page.getByRole('button', { name: 'Search', exact: true }).click();
    await page.getByRole('button', { name: 'Save view', exact: true }).click();
    const dialog = page.getByRole('dialog');
    await dialog.getByLabel('Name', { exact: true }).fill('Hypervisor selection');
    await dialog.getByRole('checkbox', { name: 'operations', exact: true }).check();
    await dialog.getByRole('checkbox', { name: 'operations may edit this definition', exact: true }).check();
    await dialog.getByRole('button', { name: 'Preview publication', exact: true }).click();
    await expect(dialog).toContainText('Publish the name, selected roots, predicates and columns to operations');
    await expect(page.getByRole('button', { name: '♧ Hypervisor selection', exact: true })).toHaveCount(0);
    await dialog.getByRole('button', { name: 'Publish reviewed definition', exact: true }).click();
    await expect(dialog).toBeHidden();
    await page.getByRole('button', { name: '♧ Hypervisor selection', exact: true }).click();
    await page.getByLabel('Search inventory').fill('Team');
    await page.getByRole('button', { name: 'Search', exact: true }).click();
    await expect(page.getByRole('button', { name: 'Team VM', exact: true })).toBeVisible();
    await page.getByRole('button', { name: 'Reset adjustments', exact: true }).click();
    await expect(page.getByLabel('Search inventory')).toHaveValue('Private');
    await page.getByLabel('Find saved views').fill('missing');
    await expect(page.getByRole('button', { name: '♧ Hypervisor selection', exact: true })).toHaveCount(0);
    await page.getByLabel('Find saved views').fill('Hypervisor');
    await expect(page.getByRole('button', { name: '♧ Hypervisor selection', exact: true })).toBeVisible();
    const limited = join(dir, 'view-reader.json');
    await writeFile(limited, JSON.stringify([{ selection: { scope: 'kinds', kinds: ['ASSET_KIND_VIRTUAL_MACHINE'] }, read: { mode: 'all' }, write: { mode: 'all' }, writable: true }]));
    await server.stop(); server = await host(dir, 'devops', limited);
    await page.goto(server.url);
    await page.getByRole('button', { name: '♧ Hypervisor selection', exact: true }).click();
    await expect(page.getByRole('button', { name: 'Private hypervisor', exact: true })).toHaveCount(0);
    await expect(page.getByRole('region', { name: 'Inventory' })).toContainText('0 resources');
  } finally { await server.stop(); await rm(dir, { recursive: true, force: true }); }
});

test('real disk inventory supports editing, context, private saves and restart', async ({ page }) => {
  // Exercise the actual UI and grpc-web host, then reopen the same database.
  const dir = await mkdtemp(join(tmpdir(), 'omloom-ui-'));
  let server = await host(dir);
  try {
    await page.goto(server.url);
    await expect(page.getByRole('heading', { name: 'Start with the infrastructure you know' })).toBeVisible();
    await create(page, 'Edge router');
    await create(page, 'Build worker', 'VIRTUAL_MACHINE');
    await page.getByRole('button', { name: 'Close resource details', exact: true }).click();
    await page.getByRole('button', { name: 'Edge router', exact: true }).click();
    await expect(page.getByRole('region', { name: 'Resource details' })).toContainText('Stable identity');
    const originalUrl = page.url();
    await page.getByRole('button', { name: 'Edit resource', exact: true }).click();
    const dialog = page.getByRole('dialog');
    await dialog.getByLabel('Name change', { exact: true }).selectOption('set');
    await dialog.getByLabel('Name', { exact: true }).fill('Edge primary');
    await dialog.getByRole('button', { name: 'Save resource', exact: true }).click();
    await expect(dialog).toBeHidden();
    await expect(page.getByRole('region', { name: 'Resource details' })).toContainText('Edge primary');
    await page.reload();
    await expect(page.getByRole('region', { name: 'Resource details' })).toContainText('Edge primary');
    await page.getByLabel('Search inventory').fill('Edge');
    await page.getByRole('button', { name: 'Search', exact: true }).click();
    await expect(page.getByRole('button', { name: 'Build worker', exact: true })).toHaveCount(0);
    await page.getByRole('button', { name: 'Save view', exact: true }).click();
    await page.getByRole('dialog').getByLabel('Name', { exact: true }).fill('Network ownership');
    await page.getByRole('button', { name: 'Save personal view', exact: true }).click();
    await expect(page.getByRole('dialog')).toBeHidden();
    await expect(page.getByRole('button', { name: '◦ Network ownership', exact: true })).toBeVisible();
    await server.stop(); server = await host(dir);
    await page.goto(server.url + new URL(originalUrl).hash);
    await expect(page.getByRole('region', { name: 'Resource details' })).toContainText('Edge primary');
    await expect(page.getByRole('button', { name: '◦ Network ownership', exact: true })).toBeVisible();
    await page.setViewportSize({ width: 390, height: 844 });
    await expect(page.getByRole('heading', { name: 'Overview', exact: true })).toBeVisible();
    await page.screenshot({ path: 'test-results/inventory-mobile.png', fullPage: true });
  } finally { await server.stop(); await rm(dir, { recursive: true, force: true }); }
});

test('departments share one backup host while namespace and host projections remain separate', async ({ page }) => {
  // Grant files are generated from actual returned IDs and selected by the host,
  // never by browser headers, query parameters or synthetic copies of a host.
  const dir = await mkdtemp(join(tmpdir(), 'omloom-departments-'));
  let server = await host(join(dir, 'db'));
  try {
    await page.goto(server.url);
    const backup = await create(page, 'Shared backup host');
    const devops = await create(page, 'DevOps hypervisor');
    const engineer = await create(page, 'Service engineering host');
    const vm = await create(page, 'Development VM', 'VIRTUAL_MACHINE');
    const support = await create(page, 'Service VM', 'VIRTUAL_MACHINE');
    const devopsStorage = await create(page, 'Operations backup namespace', 'STORAGE');
    const serviceStorage = await create(page, 'Engineering backup namespace', 'STORAGE');
    for (const storage of [devopsStorage, serviceStorage]) {
      await page.goto(`${server.url}/#/?asset=${storage.asset_id}&incarnation=${storage.incarnation}`);
      await page.getByRole('button', { name: 'Connections in broader scope', exact: true }).click();
      const dialog = page.getByRole('dialog');
      await dialog.getByRole('button', { name: 'Choose connection endpoints', exact: true }).click();
      await dialog.getByRole('combobox', { name: 'Other endpoint', exact: true }).selectOption(`${backup.asset_id}/${backup.incarnation}`);
      await dialog.getByRole('button', { name: 'Review connection', exact: true }).click();
      await dialog.getByRole('combobox', { name: 'Relationship', exact: true }).selectOption({ label: 'on' }, { timeout: 5000 });
      await dialog.getByRole('button', { name: 'Add relationship', exact: true }).click();
      await expect(dialog).toBeHidden();
      await page.getByRole('button', { name: 'Connections in broader scope', exact: true }).click();
      await dialog.getByRole('button', { name: 'Review existing connections', exact: true }).click();
      await expect(dialog).toContainText('Shared backup host');
      await dialog.getByRole('button', { name: 'Return to resource', exact: true }).click();
    }
    const grant = targets => ({ selection: { scope: 'targets', targets }, read: { mode: 'all' }, write: { mode: 'all' }, writable: true });
    const operationsFile = join(dir, 'operations.json');
    const engineeringFile = join(dir, 'engineering.json');
    const developerFile = join(dir, 'developer.json');
    await writeFile(operationsFile, JSON.stringify([grant([backup, devops, vm, devopsStorage])]));
    await writeFile(engineeringFile, JSON.stringify([grant([backup, engineer, support, serviceStorage])]));
    await writeFile(developerFile, JSON.stringify([grant([vm])]));
    await server.stop(); server = await host(join(dir, 'db'), 'devops', operationsFile);
    await page.goto(server.url);
    await expect(page.getByRole('button', { name: 'Shared backup host', exact: true })).toHaveCount(1);
    await expect(page.getByRole('button', { name: 'Operations backup namespace', exact: true })).toHaveCount(0);
    await expect(page.getByRole('button', { name: 'Engineering backup namespace', exact: true })).toHaveCount(0);
    await expect(page.getByRole('button', { name: 'Service engineering host', exact: true })).toHaveCount(0);
    await page.getByRole('button', { name: 'Shared backup host', exact: true }).click();
    expect(new URLSearchParams(new URL(page.url()).hash.split('?')[1]).get('asset')).toBe(backup.asset_id);
    await expect(page.getByRole('button', { name: 'on · Operations backup namespace', exact: true })).toBeVisible();
    await expect(page.getByRole('region', { name: 'Resource details' })).not.toContainText('Engineering backup namespace');
    await server.stop(); server = await host(join(dir, 'db'), 'engineer', engineeringFile);
    await page.goto(server.url);
    await expect(page.getByRole('button', { name: 'Shared backup host', exact: true })).toHaveCount(1);
    await expect(page.getByRole('button', { name: 'Engineering backup namespace', exact: true })).toHaveCount(0);
    await expect(page.getByRole('button', { name: 'Operations backup namespace', exact: true })).toHaveCount(0);
    await expect(page.getByRole('button', { name: 'DevOps hypervisor', exact: true })).toHaveCount(0);
    await page.getByRole('button', { name: 'Shared backup host', exact: true }).click();
    await expect(page.getByRole('button', { name: 'on · Engineering backup namespace', exact: true })).toBeVisible();
    await expect(page.getByRole('region', { name: 'Resource details' })).not.toContainText('Operations backup namespace');
    await server.stop(); server = await host(join(dir, 'db'), 'developer', developerFile);
    await page.goto(server.url);
    await expect(page.getByRole('button', { name: 'Development VM', exact: true })).toBeVisible();
    await expect(page.getByRole('button', { name: 'Shared backup host', exact: true })).toHaveCount(0);
    await page.goto(`${server.url}/#/?kind=3&actor=fixture-operator`);
    await expect(page.getByRole('button', { name: 'Shared backup host', exact: true })).toHaveCount(0);
  } finally { await server.stop(); await rm(dir, { recursive: true, force: true }); }
});

test('CSV draft revisions survive repeated previews on desktop and phone', async ({ page }) => {
  // Preview writes advance the revision; editing a draft must retain that fence and detect unrelated writes.
  for (const width of [1440,390]) {
    const dir = await mkdtemp(join(tmpdir(), 'omloom-import-revision-'));
    const server = await host(dir);
    try {
      await page.setViewportSize({width,height:900});
      await page.goto(server.url);
      if (width===390) await page.getByRole('button',{name:'More',exact:true}).click();
      await page.getByRole('button',{name:'Import CSV',exact:true}).click();
      const dialog=page.getByRole('dialog',{name:'Import inventory',exact:true});
      await dialog.getByLabel('Source name',{exact:true}).fill('first source');
      await dialog.getByLabel('CSV',{exact:true}).fill('external_id,name,other\nedge-1,Original name,Original alternative');
      await dialog.getByRole('button',{name:'Prepare preview',exact:true}).click();
      await expect(dialog.getByRole('heading',{name:'Review 1 rows',exact:true})).toBeVisible();
      await expect(dialog).toContainText('Original name');
      await dialog.getByLabel('CSV',{exact:true}).fill('external_id,name,other\nedge-1,Revised name,Final name');
      await dialog.getByRole('button',{name:'Prepare preview',exact:true}).click();
      await expect(dialog.getByRole('heading',{name:'Review 1 rows',exact:true})).toBeVisible();
      await expect(dialog).toContainText('Name: Revised name');
      await dialog.getByLabel('Source name',{exact:true}).fill('revised source');
      await dialog.getByRole('button',{name:'Prepare preview',exact:true}).click();
      await expect(dialog.getByRole('heading',{name:'Review 1 rows',exact:true})).toBeVisible();
      await expect(dialog).toContainText('Name: Revised name');
      await dialog.getByLabel('CSV column',{exact:true}).fill('other');
      await dialog.getByRole('button',{name:'Prepare preview',exact:true}).click();
      await expect(dialog.getByRole('heading',{name:'Review 1 rows',exact:true})).toBeVisible();
      await expect(dialog).toContainText('Name: Final name');
      // Repeated valid drafts must not hit the former four-preview retention ceiling.
      for (let index=0;index<3;index++) {
        await dialog.getByLabel('Source name',{exact:true}).fill(`additional source ${index}`);
        await dialog.getByRole('button',{name:'Prepare preview',exact:true}).click();
        await expect(dialog.getByRole('heading',{name:'Review 1 rows',exact:true})).toBeVisible();
        await expect(dialog).toContainText('Name: Final name');
      }
      await expect(page.getByRole('button',{name:'Final name',exact:true})).toHaveCount(0);
      await dialog.getByRole('button',{name:'Accept reviewed rows',exact:true}).click();
      await expect(dialog).toBeHidden();
      await expect(page.getByRole('button',{name:'Final name',exact:true})).toBeVisible();
      await expect(page.getByRole('button',{name:'Original name',exact:true})).toHaveCount(0);
      await expect(page.getByRole('button',{name:'Revised name',exact:true})).toHaveCount(0);
      if (!await page.getByRole('button',{name:'Import CSV',exact:true}).isVisible()) await page.getByRole('button',{name:'More',exact:true}).click();
      await page.getByRole('button',{name:'Import CSV',exact:true}).click();
      await dialog.getByLabel('Source name',{exact:true}).fill('concurrent source');
      await dialog.getByLabel('CSV',{exact:true}).fill('external_id,name\nedge-2,Before other edit');
      await dialog.getByRole('button',{name:'Prepare preview',exact:true}).click();
      await expect(dialog.getByRole('heading',{name:'Review 1 rows',exact:true})).toBeVisible();
      const other=await page.context().newPage();
      try { await other.goto(server.url); await create(other,'Unrelated edit'); } finally { await other.close(); }
      await dialog.getByLabel('CSV',{exact:true}).fill('external_id,name\nedge-2,Conflicting draft');
      await dialog.getByRole('button',{name:'Prepare preview',exact:true}).click();
      await expect(dialog.getByRole('alert')).toContainText('Code: ABORTED');
      await expect(dialog.getByRole('button',{name:'Accept reviewed rows',exact:true})).toHaveCount(0);
    } finally { await server.stop(); await rm(dir,{recursive:true,force:true}); }
  }
});

test('CSV preview validates before acceptance and scope is selected by the server', async ({ page }) => {
  // No browser login mock or API interception: both fixture hosts open real disk state.
  const dir = await mkdtemp(join(tmpdir(), 'omloom-import-ui-'));
  let server = await host(dir);
  try {
    await page.goto(server.url);
    await expect(page.getByRole('heading', { name: 'Overview', exact: true })).toBeVisible();
    await page.getByRole('button', { name: 'Import CSV', exact: true }).click();
    const dialog = page.getByRole('dialog');
    await dialog.getByLabel('Source name').fill('rack-register');
    await dialog.getByLabel('CSV', { exact: true }).fill('external_id,name\nrouter-1,Imported edge');
    await dialog.getByRole('button', { name: 'Prepare preview' }).click();
    await expect(dialog).toContainText('Create new identity');
    await expect(page.getByRole('button', { name: 'Imported edge', exact: true })).toHaveCount(0);
    await dialog.getByRole('button', { name: 'Accept reviewed rows' }).click();
    await expect(dialog).toBeHidden();
    await expect(page.getByRole('button', { name: 'Imported edge', exact: true })).toBeVisible();
    await create(page, 'App VM', 'VIRTUAL_MACHINE');
    await server.stop(); server = await host(dir, 'developer');
    await page.goto(server.url);
    await expect(page.getByRole('button', { name: 'App VM', exact: true })).toBeVisible();
    await expect(page.getByRole('button', { name: 'Imported edge', exact: true })).toHaveCount(0);
    await expect(page.getByRole('button', { name: 'Company fields', exact: true })).toHaveCount(0);
  } finally { await server.stop(); await rm(dir, { recursive: true, force: true }); }
});

test('selected roots survive view changes, personal save and reload', async ({ page }) => {
  // Saving a set of roots must not discard every root after the first or broaden scope.
  const dir = await mkdtemp(join(tmpdir(), 'omloom-root-set-'));
  const server = await host(dir);
  try {
    await page.goto(server.url);
    const firstRoot = await create(page, 'First scope host');
    const secondRoot = await create(page, 'Second scope VM', 'VIRTUAL_MACHINE');
    await create(page, 'Outside selection');
    await page.getByRole('button', { name: 'Close resource details', exact: true }).click();
    await page.getByRole('checkbox', { name: 'Select First scope host', exact: true }).check();
    await page.getByRole('checkbox', { name: 'Select Second scope VM', exact: true }).check();
    await page.getByRole('button', { name: 'Use selected as scope', exact: true }).click({ timeout: 5000 });
    await expect(page.getByRole('button', { name: 'Outside selection', exact: true })).toHaveCount(0);
    await page.getByRole('navigation', { name: 'Views' }).getByRole('button', { name: 'Software & services', exact: true }).click();
    await expect(page.getByRole('heading',{name:'No resources at this level in this view',exact:true})).toBeVisible();
    await expect(page.getByRole('button',{name:'2 selected roots',exact:true})).toBeVisible();
    await page.getByRole('button', { name: 'Save view', exact: true }).click();
    await page.getByRole('dialog').getByLabel('Name', { exact: true }).fill('Two selected roots');
    await page.getByRole('button', { name: 'Save personal view', exact: true }).click();
    await expect(page.getByRole('dialog')).toBeHidden();
    await page.goto(server.url);
    await expect(page.getByRole('button', { name: 'Outside selection', exact: true })).toBeVisible();
    await page.getByRole('button', { name: '◦ Two selected roots', exact: true }).click();
    await page.reload();
    await expect(page.getByRole('heading',{name:'Software & services',exact:true})).toBeVisible();
    await expect(page.getByRole('heading',{name:'No resources at this level in this view',exact:true})).toBeVisible();
    await expect(page.getByRole('button',{name:'2 selected roots',exact:true})).toBeVisible();
    await page.getByRole('navigation',{name:'Views'}).getByRole('button',{name:'Overview',exact:true}).click();
    // Selected roots define the enclosing level; they are not their own children.
    await expect(page.getByRole('heading',{name:'No resources at this level in this view',exact:true})).toBeVisible();
    const restored = new URLSearchParams(new URL(page.url()).hash.split('?')[1]);
    expect(restored.getAll('scope').sort()).toEqual([firstRoot.asset_id,secondRoot.asset_id].sort());
    expect(restored.getAll('scopeInc').sort()).toEqual([firstRoot.incarnation,secondRoot.incarnation].sort());
    await expect(page.getByRole('button', { name: 'First scope host', exact: true })).toHaveCount(0);
    await expect(page.getByRole('button', { name: 'Second scope VM', exact: true })).toHaveCount(0);
    await expect(page.getByRole('button', { name: 'Outside selection', exact: true })).toHaveCount(0);
  } finally { await server.stop(); await rm(dir, { recursive: true, force: true }); }
});
