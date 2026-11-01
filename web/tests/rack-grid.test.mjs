import {test} from 'node:test';
import assert from 'node:assert/strict';
import {build} from 'esbuild';
import {resolve} from 'node:path';
import {pathToFileURL} from 'node:url';
const outfile=resolve('dist/presentation/rack-grid.mjs');
await build({stdin:{contents:"export * from './web/app/rack-grid.ts'; export {PhysicalRackDirection as D} from './web/src/generated/omloom/v1/workspace.ts';",resolveDir:process.cwd()},bundle:true,platform:'node',format:'esm',packages:'external',outfile});
const {rackAllocation,rackUnitAt,adjacentRackUnit,railGrid,D}=await import(pathToFileURL(outfile).href);
const grid={topUm:30000n,unitCount:42,firstUnit:10,direction:D.BOTTOM_UP};
test('top-down pointer drops preserve the selected lower half unit',()=>{
  // Dropping a half-U body below the middle rail must not shift it into the upper half.
  const down={...grid,direction:D.TOP_DOWN};
  assert.equal(rackUnitAt(down,52225n,22225n),'10.5');
  assert.deepEqual(rackAllocation(down,rackUnitAt(down,52225n,22225n),22225n),{startUm:52225n,lengthUm:22225n});
  assert.equal(rackUnitAt(down,52224n,22225n),'10');
});
test('printed rack units use the declared rail offset and numbering direction',()=>{
  // Cabinet dimensions do not locate the rails; bottom U10 and top U51 differ.
  assert.deepEqual(rackAllocation(grid,'10',44450n),{startUm:1852450n,lengthUm:44450n});
  assert.deepEqual(rackAllocation(grid,'51',44450n),{startUm:30000n,lengthUm:44450n});
  assert.deepEqual(rackAllocation({...grid,direction:D.TOP_DOWN},'10',44450n),{startUm:30000n,lengthUm:44450n});
  assert.deepEqual(rackAllocation(grid,'10.5',22225n),{startUm:1852450n,lengthUm:22225n});
});
test('equipment height rounds outward and cannot overrun the qualified U range',()=>{
  // A short body still consumes its selected half-U allocation; fractional excess never rounds down.
  assert.equal(rackAllocation(grid,'10',44451n).lengthUm,66675n);
  assert.throws(()=>rackAllocation(grid,'51',44451n));
  assert.throws(()=>rackAllocation(grid,'9.5',1n));
  assert.throws(()=>rackAllocation(grid,'10',0n));
  assert.throws(()=>rackAllocation(grid,'10.25',1n));
});
test('pointer and keyboard selection respect physical direction for either numbering order',()=>{
  // A top-of-rack drop and an upward arrow must not select a lower physical row on a bottom-up rack.
  assert.equal(rackUnitAt(grid,30000n,88900n),'50');
  assert.equal(rackUnitAt(grid,1896899n,44450n),'10');
  assert.equal(rackUnitAt(grid,29999n,44450n),undefined);
  assert.equal(adjacentRackUnit(grid,'10',44450n,true),'11');
  const down={...grid,direction:D.TOP_DOWN};
  assert.equal(rackUnitAt(down,74450n,44450n),'11');
  assert.equal(adjacentRackUnit(down,'11',44450n,true),'10');
});
test('unknown numbering, invalid ranges and coordinate overflow are not assigned defaults',()=>{
  // Invalid declarations cannot silently acquire the usual 42U, U1 or ascending labels.
  for(const value of [{...grid,direction:0},{...grid,unitCount:0},{...grid,firstUnit:0},{...grid,firstUnit:0xffffffff},{...grid,topUm:(1n<<63n)-1n}])assert.throws(()=>railGrid(value));
});
