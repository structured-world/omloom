import {model,key,identityName,AssetKind as K,type AssetRef} from './api';
export type SceneBox={target:AssetRef;kind:number;name:string;center:[number,number,number];size:[number,number,number];root:boolean};
export type SceneLine={target:AssetRef;name:string;from:[number,number,number];to:[number,number,number]};
const axes=['x','y','z'] as const;
const max=BigInt(Number.MAX_SAFE_INTEGER);
function metres(v:bigint){if(v < -max || v>max)return undefined;return Number(v)/1_000_000;}
export function sceneData(view:model.PhysicalView,root:AssetRef){
  const items=new Map(view.items.filter(v=>v.target).map(v=>[key(v.target),v]));
  const definitions=new Map(view.definitions.map(v=>[key(v.target),v]));
  const frame=definitions.get(key(root))?.volume;
  const boxes:SceneBox[]=[],missing:model.Item[]=[],lines:SceneLine[]=[];
  const origins=new Map<string,[bigint,bigint,bigint]>();
  origins.set(key(root),frame?.x&&frame.y&&frame.z?[-frame.x.startUm,-frame.y.startUm,-frame.z.startUm]:[0n,0n,0n]);
  const bindings=new Map<string,model.PhysicalPlacement[]>();
  for(const o of view.occupancy){const p=o.placement;if(p?.item&&p.enclosure){const id=key(p.item);bindings.set(id,[...(bindings.get(id)??[]),p]);}}
  function origin(id:string,seen:Set<string>):[bigint,bigint,bigint]|undefined {
    if(origins.has(id))return origins.get(id);
    if(seen.has(id)||seen.size>=128)return undefined;seen.add(id);
    const candidates=bindings.get(id);if(candidates?.length!==1)return undefined;
    const p=candidates[0]!,volume=p.volume;if(!volume?.x||!volume.y||!volume.z)return undefined;
    const parent=origin(key(p.enclosure),seen);if(!parent)return undefined;
    // A nested usable volume can be inset from its chassis. Its minimum is a
    // fit boundary, not a translation of measured child coordinates.
    const result:[bigint,bigint,bigint]=[parent[0]+volume.x.startUm,parent[1]+volume.y.startUm,parent[2]+volume.z.startUm];origins.set(id,result);return result;
  }
  for(const [id,item]of items){
    if(item.kind===K.CABLE)continue;
    const isRoot=id===key(root),pose=isRoot?frame:bindings.get(id)?.length===1?bindings.get(id)![0]!.volume:undefined;
    const base=origin(id,new Set());
    if(!item.target||!base||!pose?.x||!pose.y||!pose.z){missing.push(item);continue;}
    const size=axes.map(a=>metres(pose[a]!.lengthUm));
    // Keep all origin arithmetic integral, including very large site offsets.
    const center=axes.map((a,i)=>{const doubled=2n*(base[i]!+(isRoot?pose[a]!.startUm:0n))+pose[a]!.lengthUm;return doubled < -max||doubled>max?undefined:Number(doubled)/2_000_000;});
    if(size.some(v=>v===undefined||v<=0)||center.some(v=>v===undefined)){missing.push(item);continue;}
    boxes.push({target:item.target,kind:item.kind,name:identityName(item),root:isRoot,size:[size[0]!,size[1]!,size[2]!],center:[center[0]!, -center[1]!, -center[2]!]});
  }
  const byTarget=new Map(boxes.map(b=>[key(b.target),b]));
  const cables=new Map<string,model.PhysicalSceneConnection[]>();
  for(const end of view.sceneConnections){const id=key(end.cable);cables.set(id,[...(cables.get(id)??[]),end]);}
  for(const [id,ends]of cables){
    // A filtered pair can be two unrelated breakout legs. Only the server can
    // qualify the complete assembly without disclosing off-scope endpoints.
    if(ends.length!==2||!ends[0]?.cable||!view.twoEndCables.some(c=>key(c)===id))continue;
    const from=byTarget.get(key(ends[0].port)),to=byTarget.get(key(ends[1]?.port));
    if(from&&to)lines.push({target:ends[0].cable,name:identityName(items.get(id)),from:from.center,to:to.center});
  }
  return{boxes,missing,lines};
}
