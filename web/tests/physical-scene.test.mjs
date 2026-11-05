import {test} from 'node:test';
import assert from 'node:assert/strict';
import {buildSync} from 'esbuild';
const built=buildSync({stdin:{contents:'export {sceneData} from "./web/app/physical-scene-data"; export {AssetKind} from "./web/src/generated/omloom/v1/contracts";',resolveDir:process.cwd()},bundle:true,write:false,format:'esm',platform:'node',define:{'location.origin':'"http://localhost"'}});
const {sceneData,AssetKind:K}=await import('data:text/javascript;base64,'+Buffer.from(built.outputFiles[0].contents).toString('base64'));
const target=id=>({assetId:id,incarnation:id});
const rack=target('rack'),server=target('server'),a=target('a'),b=target('b'),cable=target('cable');
const box=(start,length=1n)=>({x:{startUm:start,lengthUm:length},y:{startUm:start,lengthUm:length},z:{startUm:start,lengthUm:length}});
function view(){return{items:[{target:rack,kind:K.RACK,fields:[]},{target:server,kind:K.DEVICE,fields:[]}],definitions:[{target:rack,volume:box(90071992547409999n,1000n)}],occupancy:[],sceneConnections:[],twoEndCables:[]};}
test('scene origin cancellation precedes float conversion and preserves half micrometres',()=>{
  // Large surveyed site offsets must not lose small geometry or round odd lengths down.
  const v=view();v.occupancy=[{placement:{item:server,enclosure:rack,volume:box(90071992547410001n)}}];
  const result=sceneData(v,rack);
  assert.equal(result.boxes.length,2);
  assert.deepEqual(result.boxes[1].center,[.0000025,-.0000025,-.0000025]);
});
test('unknown or ambiguous placements never acquire a fabricated position',()=>{
  // A resource remains in the accessible list until one unambiguous measured binding exists.
  const v=view();assert.equal(sceneData(v,rack).missing.length,1);
  v.occupancy=[{placement:{item:server,enclosure:rack,volume:box(1n)}},{placement:{item:server,enclosure:rack,volume:box(2n)}}];
  assert.equal(sceneData(v,rack).missing.length,1);
});
test('an inset usable volume does not translate measured child anchors',()=>{
  // Correcting the usable interior cannot move a port measured from the chassis.
  const v=view(),offset=90071992547409999n;
  v.items.push({target:a,kind:K.PORT,fields:[]});
  v.definitions.push({target:server,volume:box(10n,500n)});
  v.occupancy=[{placement:{item:server,enclosure:rack,volume:box(offset+100n,600n)}},{placement:{item:a,enclosure:server,volume:box(20n,10n)}}];
  const anchor=sceneData(v,rack).boxes.find(b=>b.target===a);
  assert.deepEqual(anchor.center,[.000125,-.000125,-.000125]);
  v.definitions[1].volume=box(15n,490n);
  assert.deepEqual(sceneData(v,rack).boxes.find(b=>b.target===a).center,anchor.center);
});
test('only enclosures of displayed measured children open as cutaways',()=>{
  // Expanding a measured port opens its owning chassis, without opening a
  // closed sibling or manufacturing a location for an unknown component.
  const v=view();v.items.push({target:a,kind:K.PORT,fields:[]},{target:b,kind:K.DEVICE,fields:[]});
  const offset=90071992547409999n;
  v.occupancy=[{placement:{item:server,enclosure:rack,volume:box(offset+100n,600n)}},{placement:{item:b,enclosure:rack,volume:box(offset+800n,100n)}}];
  assert.equal(sceneData(v,rack).boxes.find(box=>box.target===server).cutaway,false);
  v.occupancy.push({placement:{item:a,enclosure:server,volume:box(20n,10n)}});
  const result=sceneData(v,rack);
  assert.equal(result.boxes.find(box=>box.target===server).cutaway,true);
  assert.equal(result.boxes.find(box=>box.target===b).cutaway,false);
  assert.equal(result.boxes.find(box=>box.target===a).cutaway,false);
});
test('cables require exactly two visible located ends and never guess breakout pairs',()=>{
  // A third named end or an unknown anchor cannot become an apparently surveyed route.
  const v=view();v.items.push({target:a,kind:K.PORT,fields:[]},{target:b,kind:K.PORT,fields:[]});
  v.occupancy=[a,b].map((item,i)=>({placement:{item,enclosure:rack,volume:box(90071992547410000n+BigInt(i))}}));
  v.sceneConnections=[{cable,port:a,endKey:'a'},{cable,port:b,endKey:'b'}];
  v.twoEndCables=[cable];
  assert.equal(sceneData(v,rack).lines.length,1);
  v.sceneConnections.push({cable,port:a,endKey:'c'});assert.equal(sceneData(v,rack).lines.length,0);
  v.sceneConnections.pop();v.occupancy.pop();assert.equal(sceneData(v,rack).lines.length,0);
});

test('two visible breakout legs without complete topology remain unpaired',()=>{
  // The third accepted end may be outside this scope. Visible cardinality is
  // insufficient to establish a two-end assembly.
  const v=view();v.items.push({target:a,kind:K.PORT,fields:[]},{target:b,kind:K.PORT,fields:[]});
  v.occupancy=[a,b].map((item,i)=>({placement:{item,enclosure:rack,volume:box(90071992547410000n+BigInt(i))}}));
  v.sceneConnections=[{cable,port:a,endKey:'leg-a'},{cable,port:b,endKey:'leg-b'}];
  v.twoEndCables=[];
  assert.equal(sceneData(v,rack).lines.length,0);
});

