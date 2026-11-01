import {test} from 'node:test';
import assert from 'node:assert/strict';
import {frameSize,projectElevation,depthOrder} from '../app/physical-elevation.ts';
const axis=(startUm,lengthUm)=>({startUm,lengthUm});
const rack={x:axis(0n,500000n),y:axis(0n,2000000n),z:axis(0n,1000000n)};
const front={x:axis(0n,250000n),y:axis(44450n,44450n),z:axis(0n,400000n)};
const rear={...front,z:axis(600000n,400000n)};

test('rear projection mirrors horizontal coordinates without mutating physical placements',()=>{
  // A front-left half-width body is on the viewer's right from behind the rack.
  assert.deepEqual(projectElevation(rack,front,'front').rect,{x:0,y:44.45,width:250,height:44.45});
  assert.deepEqual(projectElevation(rack,front,'rear').rect,{x:250,y:44.45,width:250,height:44.45});
  assert.equal(front.x.startUm,0n);
});
test('the nearest reported body paints last from the chosen face',()=>{
  // Opposing bodies may share XY; sorting by insertion order hides the wrong surface.
  for(const face of ['front','rear']){
    const projected=[front,rear].map((body,index)=>({...projectElevation(rack,body,face),index})).sort(depthOrder);
    assert.equal(projected.at(-1).index,face==='front'?0:1);
  }
});
test('large enclosure origins are subtracted before floating point conversion',()=>{
  // Micrometre differences survive an origin beyond JavaScript's safe integer range.
  const origin=9007199254740993n;
  const frame={...rack,x:axis(origin,rack.x.lengthUm),y:axis(-origin,rack.y.lengthUm)};
  const body={...front,x:axis(origin+1n,250000n),y:axis(-origin+125n,44450n)};
  assert.deepEqual(frameSize(frame),{x:0,y:0,width:500,height:2000});
  assert.deepEqual(projectElevation(frame,body,'front').rect,{x:0.001,y:0.125,width:250,height:44.45});
  assert.equal(projectElevation(frame,body,'rear').rect.x,249.999);
});
test('incomplete or unsafe geometry remains undrawn rather than acquiring default coordinates',()=>{
  // The coordinate table retains such facts; the elevation must not invent a body or depth.
  assert.equal(frameSize(undefined),undefined);
  assert.equal(projectElevation(rack,{...front,z:undefined},'front'),undefined);
  assert.equal(projectElevation(rack,{...front,x:axis(0n,0n)},'front'),undefined);
  assert.equal(frameSize({...rack,x:axis(0n,9007199254740992n)}),undefined);
});
