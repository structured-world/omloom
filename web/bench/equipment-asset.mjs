import assert from 'node:assert/strict';
import { readFile, writeFile } from 'node:fs/promises';
import { performance } from 'node:perf_hooks';
import { Box3, BoxGeometry, REVISION, Vector3 } from 'three';
import { GLTFLoader } from 'three/addons/loaders/GLTFLoader.js';

// This trial generates its own mesh. It does not decode user-supplied assets.
const [input, output] = process.argv.slice(2);
if (!input || !output) throw Error('Usage: node web/bench/equipment-asset.mjs <geometry.json> <new-report.json>');
const { boxes } = JSON.parse(await readFile(input, 'utf8'));
const chassis = boxes.find(box => box.name === 'Installed server');
assert(chassis, 'The generated projection must include its measured chassis');
const max = chassis.size.map(length => length / 2);
const min = max.map(length => -length);

function geometry() {
  const indexed = new BoxGeometry(...chassis.size);
  const triangles = indexed.toNonIndexed();
  indexed.dispose();
  return triangles;
}

function encodeGlb(positions, alter = () => {}) {
  const document = {
    asset: { version: '2.0' }, scene: 0, scenes: [{ nodes: [0] }],
    nodes: [{ mesh: 0 }], meshes: [{ primitives: [{ attributes: { POSITION: 0 } }] }],
    accessors: [{ bufferView: 0, componentType: 5126, count: positions.length / 3, type: 'VEC3', min, max }],
    bufferViews: [{ buffer: 0, byteLength: positions.byteLength }], buffers: [{ byteLength: positions.byteLength }],
  };
  alter(document);
  const json = JSON.stringify(document);
  const jsonBytes = Buffer.from(json.padEnd(Math.ceil(json.length / 4) * 4));
  const glb = new ArrayBuffer(28 + jsonBytes.length + positions.byteLength);
  const header = new DataView(glb);
  // glTF 2.0 section 4.4, JSON chunk followed by the embedded binary buffer.
  header.setUint32(0, 0x46546c67, true);
  header.setUint32(4, 2, true);
  header.setUint32(8, glb.byteLength, true);
  header.setUint32(12, jsonBytes.length, true);
  header.setUint32(16, 0x4e4f534a, true);
  new Uint8Array(glb, 20, jsonBytes.length).set(jsonBytes);
  header.setUint32(20 + jsonBytes.length, positions.byteLength, true);
  header.setUint32(24 + jsonBytes.length, 0x004e4942, true);
  const binary = new DataView(glb, 28 + jsonBytes.length);
  positions.forEach((value, index) => binary.setFloat32(index * 4, value, true));
  return glb;
}

// The trial admits only its single static primitive profile. This does not
// authorize the general loader to consume a model from an external publisher.
function checkProfile(glb) {
  assert(glb.byteLength >= 28 && glb.byteLength <= 1024 * 1024);
  const data = new DataView(glb);
  const uint = offset => data.getUint32(offset, true);
  assert.equal(uint(0), 0x46546c67);
  assert.equal(uint(4), 2);
  assert.equal(uint(8), glb.byteLength);
  const size = uint(12), binary = 20 + size;
  assert(size <= 8192 && size % 4 === 0 && binary + 8 <= glb.byteLength);
  assert.equal(uint(16), 0x4e4f534a);
  const bytes = uint(binary);
  assert.equal(uint(binary + 4), 0x004e4942);
  assert.equal(binary + 8 + bytes, glb.byteLength);
  const json = JSON.parse(new TextDecoder('utf-8', { fatal: true }).decode(new Uint8Array(glb, 20, size)));
  assert.deepEqual(Object.keys(json).sort(), ['accessors', 'asset', 'bufferViews', 'buffers', 'meshes', 'nodes', 'scene', 'scenes']);
  assert.deepEqual(json.asset, { version: '2.0' });
  assert.equal(json.scene, 0);
  assert.deepEqual(json.scenes, [{ nodes: [0] }]);
  assert.deepEqual(json.nodes, [{ mesh: 0 }]);
  assert.deepEqual(json.meshes, [{ primitives: [{ attributes: { POSITION: 0 } }] }]);
  assert.deepEqual(json.buffers, [{ byteLength: bytes }]);
  assert.deepEqual(json.bufferViews, [{ buffer: 0, byteLength: bytes }]);
  assert(bytes > 0 && bytes % 36 === 0);
  assert.deepEqual(json.accessors, [{ bufferView: 0, componentType: 5126, count: bytes / 12, type: 'VEC3', min, max }]);
  for (let offset = 0; offset < bytes; offset += 4) {
    const value = data.getFloat32(binary + 8 + offset, true), axis = offset / 4 % 3;
    assert(Number.isFinite(value) && value >= min[axis] - 0.000001 && value <= max[axis] + 0.000001);
  }
}

const source = geometry();
const glb = encodeGlb(source.attributes.position.array);
checkProfile(glb);
assert.throws(() => checkProfile(glb.slice(0, -4)), 'truncated container');
assert.throws(() => checkProfile(new ArrayBuffer(1024 * 1024 + 1)), 'oversized container');
assert.throws(() => checkProfile(encodeGlb(source.attributes.position.array, doc => {
  doc.buffers[0].uri = 'https://example.invalid/mesh.bin';
})), 'external resource');
assert.throws(() => checkProfile(encodeGlb(source.attributes.position.array, doc => {
  doc.accessors[0].count = 2 ** 32;
})), 'oversized accessor');
assert.throws(() => checkProfile(encodeGlb(source.attributes.position.array, doc => {
  doc.extensionsRequired = ['EXT_meshopt_compression'];
})), 'unqualified extension');
source.dispose();
const loader = new GLTFLoader();
const procedural = [], embedded = [];
for (let i = 0; i < 40; i++) {
  let start = performance.now();
  const box = geometry();
  if (i >= 10) procedural.push(performance.now() - start);
  box.dispose();
  start = performance.now();
  const { scene } = await loader.parseAsync(glb, '');
  if (i >= 10) embedded.push(performance.now() - start);
  const dimensions = new Box3().setFromObject(scene).getSize(new Vector3()).toArray();
  assert(dimensions.every((value, axis) => Math.abs(value - chassis.size[axis]) < 0.000001));
  scene.traverse(node => {
    if (node.isMesh) {
      node.geometry.dispose();
      node.material.dispose();
    }
  });
}
const summarize = samples => {
  const sorted = [...samples].sort((a, b) => a - b);
  return Object.fromEntries([50, 95, 99].map(p => [`p${p}`, sorted[Math.ceil(samples.length * p / 100) - 1]]));
};
await writeFile(output, JSON.stringify({
  threeRevision: REVISION, glbBytes: glb.byteLength, dimensionsMetres: chassis.size,
  proceduralMs: summarize(procedural), glbDecodeMs: summarize(embedded),
  extensions: [], externalResources: 0, rejectedCases: 5, samples: procedural.length,
}, null, 2) + '\n', { flag: 'wx' });
