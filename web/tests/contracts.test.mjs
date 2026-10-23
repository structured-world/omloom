import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync, writeFileSync } from 'node:fs';
import { decodeWire, Record, OperationState, EffectOutcome } from '../../dist/codec.js';

const root = new URL('../../dist/corpus/', import.meta.url);
const manifest = readFileSync(new URL('manifest.tsv', root), 'utf8').trim().split('\n');
const wireErrors = new Set(['Wire', 'UnknownField', 'DuplicateField', 'Limit']);

for (const row of manifest) {
  const [name, expected] = row.split('\t');
  test(`protobuf interoperability: ${name}`, () => {
    // Wire-profile errors must match Rust before generated decode; semantic
    // admission remains server-side and is verified by the Rust corpus.
    const bytes = new Uint8Array(readFileSync(new URL(`${name}.bin`, root)));
    if (wireErrors.has(expected)) {
      assert.throws(() => decodeWire(bytes), { message: expected });
      return;
    }
    const record = decodeWire(bytes);
    const encoded = Record.toBinary(record);
    assert.deepEqual(encoded, bytes);
    if (expected === 'ok') writeFileSync(new URL(`${name}.ts.bin`, root), encoded);
    if (name === 'asset') {
      assert.equal(record.body.oneofKind, 'asset');
      assert.equal(record.body.asset.revision, 0xffff_ffff_ffff_ffffn);
      const evidence = record.body.asset.observed[0].evidence;
      assert.equal(evidence.fields[1].value.oneofKind, 'clear');
      assert.equal(evidence.fields[2].value.flag, false);
      assert.equal(evidence.fields[3].value.integer, -0x8000_0000_0000_0000n);
      assert.equal(evidence.sourceRevision, undefined);
      assert.notEqual(record.body.asset.intended[0].field.value.text, evidence.fields[0].value.text);
    }
    if (name === 'partial_refresh' || name === 'partial_import') {
      // A missing field keeps its old window while an explicit clear and false
      // remain present in the refreshed group. This is current state, not samples.
      const asset = record.body.asset;
      const groups = name === 'partial_import' ? asset.imported : asset.observed;
      assert.equal(groups.length, 2);
      const [old, fresh] = groups.map(group => group.evidence);
      assert.equal(old.sourceId, fresh.sourceId);
      assert.deepEqual(old.fields.map(field => field.name), ['name', 'delta']);
      assert.deepEqual(fresh.fields.map(field => field.name), ['description', 'enabled']);
      assert.deepEqual([old.observedAtMs, old.validUntilMs, fresh.observedAtMs, fresh.validUntilMs], [100n, 200n, 300n, 400n]);
      assert.equal(fresh.fields[0].value.oneofKind, 'clear');
      assert.equal(fresh.fields[1].value.flag, false);
      assert.equal(asset.intended[0].field.value.text, 'accepted intent');
    }
    if (name === 'scan_full_page') {
      // Check actual serialized overhead, not just generated interface names.
      const scan = record.body.scan;
      assert.equal(scan.candidates.length, 128);
      for (const context of [scan.connectionId, scan.scanId]) {
        assert.equal(Buffer.from(encoded).toString('hex').split(Buffer.from(context).toString('hex')).length - 1, 1);
      }
      for (const row of scan.candidates) {
        assert.equal(row.evidence.observedAtMs, 100n);
        assert.equal(row.evidence.validUntilMs, 200n);
        assert.equal(row.evidence.fields[1].value.oneofKind, 'clear');
      }
    }
    if (name === 'operation') {
      assert.equal(record.body.operation.state, OperationState.RECONCILING);
      assert.equal(record.body.operation.outcome, EffectOutcome.UNKNOWN);
    }
  });
}

test('browser input hard size ceiling', () => {
  // Oversized data is rejected before the reflection decoder allocates values.
  assert.throws(() => decodeWire(new Uint8Array(1_048_577)), { message: 'Limit' });
});
