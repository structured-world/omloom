import { GrpcWebFetchTransport } from '@protobuf-ts/grpcweb-transport';
import { Request, Workspace, FieldType, type Reply, type WorkingContext } from '../src/generated/omloom/v1/workspace';
import { AssetKind, NullValue, type Field, type AssetRef } from '../src/generated/omloom/v1/contracts';
import { softwareChoiceLabel } from './software-fields';
export * as model from '../src/generated/omloom/v1/workspace';
export { AssetKind, RelationKind, NullValue, OperationState, EffectOutcome } from '../src/generated/omloom/v1/contracts';
export type { Field, AssetRef, Relation } from '../src/generated/omloom/v1/contracts';
const transport = new GrpcWebFetchTransport({ baseUrl: location.origin, format: 'binary' });
export async function call(command: Request['command'], working?: WorkingContext): Promise<Reply['result']> {
  const method = Workspace.methods[0];
  if (!method) throw Error('Workspace service metadata is missing');
  const { response } = await transport.unary<Request, Reply>(method, Request.create({ command, working }), transport.mergeOptions({ timeout: 15_000 }));
  return response.result;
}
export function text(name: string, value: string): Field { return { name, value: { oneofKind: 'text', text: value } }; }
export function typedField(name: string, type: FieldType, value: string, clear = false): Field {
  if (clear) return { name, value: { oneofKind: 'clear', clear: NullValue.NULL_VALUE } };
  if (type === FieldType.INTEGER) {
    if (!/^-?(0|[1-9][0-9]*)$/.test(value)) throw Error('Enter an integer explicitly. Empty input is not zero.');
    return { name, value: { oneofKind: 'integer', integer: BigInt(value) } };
  }
  if (type === FieldType.BOOLEAN) {
    if (value !== 'true' && value !== 'false') throw Error('Select true or false explicitly.');
    return { name, value: { oneofKind: 'flag', flag: value === 'true' } };
  }
  return text(name, value);
}
export function display(field: Field | undefined): string {
  if (field?.value.oneofKind === 'clear') return '(cleared)';
  if (field?.value.oneofKind === 'text' && field.value.text === '') return '(empty)';
  return field ? softwareChoiceLabel(field.name, input(field)) : '';
}
export function input(field: Field | undefined): string {
  const v = field?.value;
  if (!v || !v.oneofKind) return '';
  if (v.oneofKind === 'clear') return '';
  if (v.oneofKind === 'text') return v.text;
  if (v.oneofKind === 'integer') return String(v.integer);
  if (v.oneofKind === 'equipment') {
    const claim=v.equipment;const a=claim.assertion;
    let value='Unresolved';
    if(a.oneofKind==='quantity'){
      const q=a.quantity;
      const number=(n:bigint|undefined)=>{
        if(n===undefined)return '?';const s=n.toString().padStart(q.scale+1,'0');
        return q.scale ? s.slice(0,-q.scale)+'.'+s.slice(-q.scale):s;
      };
      const unit=field?.name==='drive_bays'?'bays':q.unit;
      if(q.upper===undefined)value=`At least ${number(q.lower)} ${unit}; ${unit==='bays'?'total':'upper bound'} unknown`;
      else if(q.lower===undefined)value=`At most ${number(q.upper)} ${unit}; lower bound unknown`;
      else if(q.lower===q.upper)value=`Known: ${number(q.lower)} ${unit}`;
      else value=`Known range: ${number(q.lower)}–${number(q.upper)} ${unit}`;
    }else if(a.oneofKind==='model')value=`Known reference: ${a.model.issuer} / ${a.model.key} (${a.model.basis})`;
    else if(a.oneofKind==='alternatives'){
      const set=a.alternatives;
      const reference=(r:import('../src/generated/omloom/v1/contracts').EquipmentReference|undefined)=>r?` [${r.issuer} / ${r.key} (${r.basis})]`:'';
      const possible=set.candidates.map(c=>c.value+reference(c.reference)+(c.probabilityPpm!==undefined?` (attributed probability ${c.probabilityPpm/10000}%)`:c.scorePpm!==undefined?` (uncalibrated score ${c.scorePpm/1000000})`:''));
      const excluded=set.excluded.map(e=>`${e.value}${reference(e.reference)}, because ${e.reason}; coverage: ${e.coverage}`);
      const conflict=set.candidates.some(c=>set.excluded.some(e=>e.value===c.value&&reference(e.reference)===reference(c.reference)));
      value=[possible.length?'Possible: '+possible.join(', '):'',excluded.length?'Excluded: '+excluded.join('; '):'',set.exhaustive?'Declared complete domain':'Incomplete alternatives; other models remain unknown',set.residualPpm!==undefined?`Unknown probability: ${set.residualPpm/10000}%`:'',set.domain?`Domain: ${set.domain}`:'',set.calibrationBasis?`Attributed calibration: ${set.calibrationBasis}`:'',conflict?'Conflicting claims require review':''].filter(Boolean).join(' · ');
    }
    return `${value} · Basis: ${claim.basis}`;
  }
  return String(v.flag);
}
export function key(target: AssetRef | undefined): string { return target ? `${target.assetId}/${target.incarnation}` : ''; }
export function enumLabel(names: Record<number, string>, value: number): string {
  const name = names[value];
  return name === undefined ? `unknown (${value})` : name.toLowerCase();
}
export function label(kind: number): string { return AssetKind[kind]?.replace('ASSET_KIND_', '').toLowerCase().replaceAll('_', ' ') ?? 'Unavailable'; }
export const kinds = Object.values(AssetKind).filter((k): k is number => typeof k === 'number' && k > 0);
