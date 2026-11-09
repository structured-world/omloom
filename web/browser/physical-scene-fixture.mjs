import { expect } from '@playwright/test';
import { workspaceClient } from './workspace-client.mjs';

export async function fixture(url, pairs = 1){
  if (!Number.isInteger(pairs) || pairs < 1 || pairs > 12) throw Error('Fixture supports 1 to 12 cable pairs');
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
  const ports = [a, b], cables = [cable];
  for (let i = 1; i < pairs; i++) {
    ports.push(await create(m.AssetKind.PORT, `Additional port ${i} A`));
    ports.push(await create(m.AssetKind.PORT, `Additional port ${i} B`));
    cables.push(await create(m.AssetKind.CABLE, `Patch cable ${i}`));
  }
  await physical({oneofKind:'define',define:{definition:{target:rack,basis:'Measured enclosure',volume:volume(0n,0n,0n,600000n,2200000n,1000000n)}}});
  await physical({oneofKind:'define',define:{definition:{target:host,basis:'Measured chassis frame',volume:volume(0n,0n,0n,500000n,200000n,850000n)}}});
  async function actual(steps){const {plan}=await physical({oneofKind:'save',save:{plan:{name:'Recorded inspection',steps}}});await physical({oneofKind:'complete',complete:{plan:{id:plan.id,revision:plan.revision},actualState:true,discrepancy:'Actual inspection; no live connectivity qualification',completedSteps:steps.map((_,i)=>i)}});}
  await actual([{action:{oneofKind:'placement',placement:{item:host,enclosure:rack,volume:volume(50000n,300000n,50000n,500000n,200000n,850000n)}}},{action:{oneofKind:'placement',placement:{item:unknown,enclosure:rack,volume:volume(50000n,700000n,50000n,500000n,200000n,850000n)}}}]);
  for(const port of ports)await rpc({oneofKind:'relation',relation:m.Relationship.create({relation:{from:host,to:port,kind:m.RelationKind.HAS_PORT}})});
  const detail=await rpc({oneofKind:'read',read:m.Read.create({target:host})});
  for(const [i,port]of ports.entries()){
    const binding=detail.detail.relations.find(r=>r.to?.assetId===port.assetId&&r.kind===m.RelationKind.HAS_PORT);
    expect(binding).toBeDefined();
    const pose = i < 2
      ? volume(BigInt(40000+i*300000),60000n,0n,40000n,30000n,20000n)
      : volume(BigInt(20000+(i-2)%8*55000),BigInt(105000+Math.floor((i-2)/8)*26000),0n,40000n,16000n,20000n);
    await physical({oneofKind:'portAnchor',portAnchor:{bindingId:binding.relationId,expectedBindingRevision:binding.revision,volume:pose}});
  }
  for(const cable of cables)for(const end of ['a','b'])await physical({oneofKind:'cableEnd',cableEnd:{cable,end:{key:end,name:end,basis:'Labelled inspected cable'}}});
  await actual(ports.map((port,i)=>({action:{oneofKind:'wiring',wiring:{cable:cables[Math.floor(i/2)],endKey:i%2?'b':'a',basis:{oneofKind:'expectedEndRevision',expectedEndRevision:1n},intent:m.PhysicalWiringIntent.PRESERVE,after:{oneofKind:'port',port}}}})));
  return{m,rpc,rack,host,unknown,a,b,cable,ports,cables};
}
