import type {PhysicalVolume} from '../src/generated/omloom/v1/workspace';

export type Face='front'|'rear';
const safe=BigInt(Number.MAX_SAFE_INTEGER);
function mm(value:bigint){if(value < -safe || value > safe)return undefined;return Number(value)/1000;}
export function frameSize(volume:PhysicalVolume|undefined){
  if(!volume?.x||!volume.y||!volume.z||volume.x.lengthUm<=0n||volume.y.lengthUm<=0n||volume.z.lengthUm<=0n)return undefined;
  const width=mm(volume.x.lengthUm),height=mm(volume.y.lengthUm);
  if(width===undefined||height===undefined)return undefined;
  return {x:0,y:0,width,height};
}
// Subtract the recorded origin as integers before entering SVG's floating-point frame.
export function projectElevation(frame:PhysicalVolume,body:PhysicalVolume,face:Face){
  if(!frameSize(frame)||!frame.x||!frame.y||!frame.z||!body.x||!body.y||!body.z)return undefined;
  if(body.x.lengthUm<=0n||body.y.lengthUm<=0n||body.z.lengthUm<=0n)return undefined;
  const localX=body.x.startUm-frame.x.startUm;
  const x=mm(face==='front'?localX:frame.x.lengthUm-localX-body.x.lengthUm);
  const y=mm(body.y.startUm-frame.y.startUm),width=mm(body.x.lengthUm),height=mm(body.y.lengthUm);
  if(x===undefined||y===undefined||width===undefined||height===undefined)return undefined;
  const z=body.z.startUm-frame.z.startUm;
  return {rect:{x,y,width,height},depth:face==='front'?-z:z+body.z.lengthUm};
}
export function depthOrder(a:{depth:bigint},b:{depth:bigint}){
  if(a.depth<b.depth)return -1;
  if(a.depth>b.depth)return 1;
  return 0;
}
