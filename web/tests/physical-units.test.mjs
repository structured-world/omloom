import { test } from 'node:test';
import assert from 'node:assert/strict';
import { micrometres, nonnegativeMicrometres, signedMicrometres, millimetres, upperMicrometres } from '../app/physical-units.ts';

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
  for (const input of ['', '0', '-1', '1.0001', '01', '1e3', 'Infinity', ' 1', '9223372036854775.808']) {
    assert.throws(()=>micrometres(input));
  }
});
test('physical entry covers the complete signed storage coordinate range',()=>{
  // Existing endpoint values must round-trip without rejecting valid magnitudes or accepting overflow.
  const maximum=(1n<<63n)-1n,minimum=-(1n<<63n);
  assert.equal(micrometres(millimetres(maximum)),maximum);
  assert.equal(signedMicrometres(millimetres(minimum)),minimum);
  assert.equal(signedMicrometres(millimetres(maximum)),maximum);
  assert.throws(()=>signedMicrometres(millimetres(minimum-1n)));
  assert.throws(()=>micrometres(millimetres(maximum+1n)));
  assert.equal(micrometres('99999999999999'),99999999999999000n);
});
test('physical offsets retain the sign for submillimetre clearances', () => {
  // Clearance envelopes may start outside the chassis; their offset is never a dimension input.
  assert.equal(millimetres(-125n),'-0.125');
  assert.equal(millimetres(-1125n),'-1.125');
  assert.equal(signedMicrometres('-0.125'),-125n);
  assert.equal(signedMicrometres('0'),0n);
  assert.equal(signedMicrometres('1.125'),1125n);
  for(const value of ['','--1','-','-1.0001','+1'])assert.throws(()=>signedMicrometres(value));
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

test('equipment envelope cannot scale a valid quantity past signed storage bounds',()=>{
  // Unit conversion must reject an oversized reservation before serialization, without clamping it.
  const maximum=(1n<<63n)-1n;
  assert.throws(()=>upperMicrometres({upper:9223372036854776n,unit:'mm',scale:0}));
  for(const scale of [0,1,2,3]){
    const factor=10n**BigInt(3-scale),upper=maximum/factor;
    assert.equal(upperMicrometres({upper,unit:'mm',scale}),upper*factor);
    assert.throws(()=>upperMicrometres({upper:upper+1n,unit:'mm',scale}));
  }
  for(const scale of [4,5,6]){
    const divisor=10n**BigInt(scale-3);
    assert.equal(upperMicrometres({upper:maximum,unit:'mm',scale}),maximum/divisor+1n);
  }
});
