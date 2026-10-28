import { Request, type HostReport } from '../src/generated/omloom/v1/workspace';
import { preflightMessage } from '../src/codec';

/** Native file envelope only. This never supplies a working context or grants. */
export function readHostReport(bytes: Uint8Array): HostReport {
  // Reserve 1152 of the RPC's 4096 field visits for 128 matched selections,
  // 128 scope roots, focus and section selectors before creating any objects.
  preflightMessage(bytes, Request, 262144, 2944);
  const request = Request.fromBinary(bytes, { readUnknownField: 'throw' });
  if (request.command.oneofKind !== 'hostImport' || !request.command.hostImport.report) {
    throw Error('Choose a native Omloom host report.');
  }
  const report = request.command.hostImport.report;
  if (report.version !== 1 || report.findings.length > 128 || report.observedAt <= 0n || report.observedAt > 8640000000000000n) {
    throw Error('Unsupported report version, finding count or observation time.');
  }
  return report;
}
