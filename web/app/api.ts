import { GrpcWebFetchTransport } from '@protobuf-ts/grpcweb-transport';
import { Request, Workspace, type Reply } from '../src/generated/omloom/v1/workspace';
import { AssetKind, type Field, type AssetRef } from '../src/generated/omloom/v1/contracts';
export * as model from '../src/generated/omloom/v1/workspace';
export { AssetKind, RelationKind, NullValue } from '../src/generated/omloom/v1/contracts';
export type { Field, AssetRef, Relation } from '../src/generated/omloom/v1/contracts';
const transport = new GrpcWebFetchTransport({ baseUrl: location.origin, format: 'binary' });
export async function call(command: Request['command']): Promise<Reply['result']> {
  const method = Workspace.methods[0];
  if (!method) throw Error('Workspace service metadata is missing');
  const { response } = await transport.unary<Request, Reply>(method, Request.create({ command }), transport.mergeOptions({ timeout: 15_000 }));
  return response.result;
}
export function text(name: string, value: string): Field { return { name, value: { oneofKind: 'text', text: value } }; }
export function display(field: Field | undefined): string {
  if (field?.value.oneofKind === 'clear') return '(cleared)';
  if (field?.value.oneofKind === 'text' && field.value.text === '') return '(empty)';
  return input(field);
}
export function input(field: Field | undefined): string {
  const v = field?.value;
  if (!v || !v.oneofKind) return '';
  if (v.oneofKind === 'clear') return '';
  if (v.oneofKind === 'text') return v.text;
  if (v.oneofKind === 'integer') return String(v.integer);
  return String(v.flag);
}
export function key(target: AssetRef | undefined): string { return target ? `${target.assetId}/${target.incarnation}` : ''; }
export function label(kind: number): string { return AssetKind[kind]?.replace('ASSET_KIND_', '').toLowerCase().replaceAll('_', ' ') ?? 'Unavailable'; }
export const kinds = Object.values(AssetKind).filter((k): k is number => typeof k === 'number' && k > 0);
