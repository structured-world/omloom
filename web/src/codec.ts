import { Record } from './generated/omloom/v1/contracts.js';
import { ScalarType, RepeatType, type IMessageType } from '@protobuf-ts/runtime';

/** Shared hard wire profile for records and native report envelopes. Server
 * domain validation/authorization is never a browser claim.
 * Generated JSON helpers are not untrusted admission entry points. */
export function preflightMessage(bytes: Uint8Array, root: IMessageType<object>, byteLimit = 1_048_576, fieldLimit = 4096): void {
  if (bytes.byteLength > Math.min(byteLimit, 1_048_576)) throw new Error('Limit');
  const maxFields = Math.min(fieldLimit, 4096);
  let fields = 0;
  let charge = bytes.byteLength * 2 + 4096;
  function chargeField(): void {
    if (++fields > maxFields) throw new Error('Limit');
    charge += 2048;
    if (charge > 16 * 1_048_576) throw new Error('Limit');
  }
  function message(start: number, end: number, type: IMessageType<object>, depth: number): void {
    if (depth > 16) throw new Error('Limit');
    let at = start;
    let seen = 0n;
    const groups = new Set<string>();
    function varint(): bigint {
      let result = 0n;
      for (let shift = 0n; shift < 70n; shift += 7n) {
        if (at === end) throw new Error('Wire');
        const b = bytes[at++]!;
        if (shift === 63n && b > 1) throw new Error('Wire');
        result |= BigInt(b & 127) << shift;
        if (b < 128) return result;
      }
      throw new Error('Wire');
    }
    while (at < end) {
      chargeField();
      const key = varint();
      const tag = key >> 3n;
      if (tag > 0xffff_ffffn) throw new Error('Wire');
      const info = type.fields.find(f => BigInt(f.no) === tag);
      if (!info) throw new Error('UnknownField');
      const mask = 1n << tag;
      if (info.repeat === RepeatType.NO && (seen & mask) !== 0n) throw new Error('DuplicateField');
      seen |= mask;
      if (info.oneof) {
        if (groups.has(info.oneof)) throw new Error('DuplicateField');
        groups.add(info.oneof);
      }
      const max = info.kind === 'enum' ? 0x7fff_ffffn
        : info.kind === 'scalar' && info.T === ScalarType.BOOL ? 1n
        : info.kind === 'scalar' && info.T === ScalarType.UINT32 ? 0xffff_ffffn
        : 0xffff_ffff_ffff_ffffn;
      const scalar = info.kind === 'enum' || (info.kind === 'scalar' && ![ScalarType.STRING, ScalarType.BYTES].includes(info.T));
      if ((key & 7n) === 0n && scalar) {
        if (varint() > max) throw new Error('Wire');
      } else if ((key & 7n) === 2n) {
        const len = varint();
        if (len > BigInt(end - at)) throw new Error('Wire');
        const stop = at + Number(len);
        if (info.kind === 'message') {
          message(at, stop, info.T(), depth + 1);
          at = stop;
        } else if (scalar && info.repeat !== RepeatType.NO) {
          while (at < stop) {
            chargeField();
            if (varint() > max || at > stop) throw new Error('Wire');
          }
        } else if (info.kind === 'scalar' && info.T === ScalarType.STRING) {
          // Fatal UTF-8 decoding is done by the generated reader after preflight.
          at = stop;
        } else if (info.kind === 'scalar' && info.T === ScalarType.BYTES) {
          at = stop;
        } else throw new Error('Wire');
      } else throw new Error('Wire');
    }
  }
  message(0, bytes.byteLength, root, 1);
}

export function decodeWire(bytes: Uint8Array): Record {
  preflightMessage(bytes, Record);
  try { return Record.fromBinary(bytes, { readUnknownField: 'throw' }); }
  catch { throw new Error('Wire'); }
}

/** Enum labels always come from the generated reverse mapping at the caller. */
export { Record, OperationState, EffectOutcome, AssetKind } from './generated/omloom/v1/contracts.js';
