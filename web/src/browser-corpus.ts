import { decodeWire, Record, OperationState } from './codec.js';

// Verification harness only; this is not a product UI or login implementation.
Object.assign(globalThis, {
  checkContract(bytes: number[]) {
    const value = decodeWire(Uint8Array.from(bytes));
    const encoded = Record.toBinary(value);
    const state = value.body.oneofKind === 'operation' ? OperationState[value.body.operation.state] : null;
    return { bytes: Array.from(encoded), state, json: Record.toJsonString(value) };
  }
});
