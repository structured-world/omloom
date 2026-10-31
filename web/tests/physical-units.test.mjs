import { test } from 'node:test';
import assert from 'node:assert/strict';
import { micrometres, nonnegativeMicrometres, millimetres, upperMicrometres } from '../app/physical-units.ts';

test('physical entry preserves exact decimal units without floating point rounding', () => {
  // A rendered millimetre value round-trips to the authoritative micrometre integer.
  for (const [input, value, display] of [['482.600',482600n,'482.6'],['0.001',1n,'0.001'],['1000.125',1000125n,'1000.125'],['9999999999999.999',9999999999999999n,'9999999999999.999']]) {
    assert.equal(micrometres(input),value);
    assert.equal(millimetres(value),display);
    assert.equal(micrometres(millimetres(value)),value);
  }
});
test('physical entry rejects zero, overflow length and unsupported precision before submission', () => {
  // Missing/invalid dimensions must not acquire an implicit zero or rounded envelope.
  for (const input of ['', '0', '-1', '1.0001', '01', '1e3', 'Infinity', ' 1', '99999999999999']) {
    assert.throws(()=>micrometres(input));
  }
});
test('physical offsets retain the sign for submillimetre clearances', () => {
  // Clearance envelopes may start outside the chassis; their offset is never a dimension input.
  assert.equal(millimetres(-125n),'-0.125');
  assert.equal(millimetres(-1125n),'-1.125');
});
test('a declared clearance may be zero but never absent, negative or rounded',()=>{
  // Explicit zero margins differ from missing measurements; ordinary body dimensions remain positive.
  assert.equal(nonnegativeMicrometres('0'),0n);
  assert.equal(nonnegativeMicrometres('0.000'),0n);
  assert.equal(nonnegativeMicrometres('0.001'),1n);
  for(const input of ['','-1','1.0001','01',' 0','1e3'])assert.throws(()=>nonnegativeMicrometres(input));
  assert.throws(()=>micrometres('0'));
});
test('equipment envelope uses the known upper bound and rounds outward',()=>{
  // A range or submicrometre remainder cannot silently shrink a future reservation.
  assert.equal(upperMicrometres({upper:440n,unit:'mm',scale:0}),440000n);
  assert.equal(upperMicrometres({upper:4400001n,unit:'mm',scale:4}),440001n);
  assert.equal(upperMicrometres({unit:'mm',scale:0}),undefined);
  assert.equal(upperMicrometres({upper:10n,unit:'g',scale:0}),undefined);
});
