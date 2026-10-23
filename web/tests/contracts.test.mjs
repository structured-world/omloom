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
      assert.equal(record.body.asset.observed[0].fields[1].value.oneofKind, 'clear');
      assert.equal(record.body.asset.observed[0].fields[2].value.flag, false);
      assert.equal(record.body.asset.observed[0].fields[3].value.integer, -0x8000_0000_0000_0000n);
      assert.equal(record.body.asset.observed[0].provenance.sourceRevision, undefined);
      assert.notEqual(record.body.asset.intended[0].field.value.text, record.body.asset.observed[0].fields[0].value.text);
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
