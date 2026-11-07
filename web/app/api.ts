import { GrpcWebFetchTransport } from '@protobuf-ts/grpcweb-transport';
import { Request, Workspace, FieldType, EvaluateRequirements, RequirementGroupKind, type FieldRequirement, type Item, type Reply, type WorkingContext } from '../src/generated/omloom/v1/workspace';
import { AssetKind, NullValue, type Field, type AssetRef, type EquipmentAlternative, type EquipmentExclusion, type EquipmentReference } from '../src/generated/omloom/v1/contracts';
import { softwareChoiceLabel } from './software-fields';
export * as model from '../src/generated/omloom/v1/workspace';
export { AssetKind, RelationKind, NullValue, OperationState, EffectOutcome } from '../src/generated/omloom/v1/contracts';
export type { Field, AssetRef, Relation } from '../src/generated/omloom/v1/contracts';
const transport = new GrpcWebFetchTransport({ baseUrl: location.origin, format: 'binary' });
async function page(command: Request['command'], working?: WorkingContext, abort?: AbortSignal): Promise<Reply['result']> {
  const method = Workspace.methods[0];
  if (!method) throw Error('Workspace service metadata is missing');
  const { response } = await transport.unary<Request, Reply>(method, Request.create({ command, working }), transport.mergeOptions({ timeout: 15_000, abort }));
  return response.result;
}
export async function call(command: Request['command'], working?: WorkingContext, abort?: AbortSignal): Promise<Reply['result']> {
  const response = await page(command, working, abort);
  if (command.oneofKind !== 'evaluateRequirements' || command.evaluateRequirements.cursor !== undefined || response.oneofKind !== 'requirementEvaluation') return response;
  const evaluation = response.requirementEvaluation;
  // Publish only complete evidence. The server rejects continuations when
  // authority, values, freshness decisions or the question's context change.
  for (let pages = 0; evaluation.next !== undefined; pages++) {
    if (pages >= 2048) throw Error('The evidence continuation exceeds the supported source count.');
    const reply = await page({ oneofKind: 'evaluateRequirements', evaluateRequirements: { ...command.evaluateRequirements, cursor: evaluation.next } }, working);
    if (reply.oneofKind !== 'requirementEvaluation') throw Error('The remaining evidence is unavailable. Retry the check.');
    for (const node of reply.requirementEvaluation.results) {
      const original = evaluation.results.find(result => result.node === node.node);
      if (!original) throw Error('The evidence no longer matches this question. Retry the check.');
      original.evidence.push(...node.evidence);
    }
    evaluation.next = reply.requirementEvaluation.next;
  }
  return response;
}
export function isAccessFailure(error: unknown) {
  return error instanceof Error && 'code' in error && (error.code === 'PERMISSION_DENIED' || error.code === 'NOT_FOUND');
}
export async function evaluateFieldGroup(fields: FieldRequirement[], working: WorkingContext, options: {
  retryAnchors?: boolean;
  preferredAnchor?: string;
  onAccessFailure?: () => boolean | void;
} = {}) {
  const query = EvaluateRequirements.create({ anchor: fields[0]?.target, root: fields.length, nodes: [
    ...fields.map(field => ({ expression: { oneofKind: 'field' as const, field } })),
    { expression: { oneofKind: 'group', group: { kind: RequirementGroupKind.ALL, minimum: 0, members: fields.map((_, index) => index) } } },
  ] });
  if (options.preferredAnchor) query.anchor = fields.find(field => key(field.target) === options.preferredAnchor)?.target ?? query.anchor;
  let index = 0;
  let tried: Set<string> | undefined;
  for (;;) {
    try {
      const reply = await call({ oneofKind: 'evaluateRequirements', evaluateRequirements: query }, working);
      return { reply, anchor: key(query.anchor) };
    } catch (error) {
      if (!isAccessFailure(error) || options.onAccessFailure?.() === false || options.retryAnchors === false) throw error;
      // Only the anchor changes. Membership and the server's scope checks stay fixed.
      tried ??= new Set([key(query.anchor)]);
      while (index < fields.length && tried.has(key(fields[index]!.target))) index++;
      if (index === fields.length) throw error;
      query.anchor = fields[index++]!.target;
      tried.add(key(query.anchor));
    }
  }
}
export function requestMessage(error:unknown):string {
  if(!(error instanceof Error))return 'The request could not be completed.';
  if('code' in error&&error.code==='PERMISSION_DENIED')return 'This request is unavailable or outside your current permissions.';
  try{return decodeURIComponent(error.message);}catch{return error.message;}
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
export function resourceName(item: Item | undefined, domain: Field[] | string = []): string {
  return input(item?.fields.find(field => field.name === 'name'))
    || input(item?.fields.find(field => field.name === 'cidr'))
    || (typeof domain === 'string' ? domain : input(domain.find(field => field.name === 'cidr')));
}
export function identityName(item: Item | undefined, domain: Field[] | string = []): string {
  const name = input(item?.fields.find(field => field.name === 'name'));
  if (name) return name;
  if (!item?.target) return '';
  return `${resourceName(item, domain) || label(item.kind)} (${item.target.assetId.slice(-8)})`;
}
function referenceLabel(value:EquipmentReference):string {
  return `issuer: ${JSON.stringify(value.issuer)}; key: ${JSON.stringify(value.key)}; basis: ${JSON.stringify(value.basis)}`;
}
export function input(field: Field | undefined): string {
  if (!field) return '';
  const v = field.value;
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
    }else if(a.oneofKind==='model')value=`Known reference: ${referenceLabel(a.model)}`;
    else if(a.oneofKind==='alternatives'){
      const set=a.alternatives;
      const reference=(r:EquipmentReference|undefined)=>r?` [${referenceLabel(r)}]`:'';
      const possible=set.candidates.map(c=>c.value+reference(c.reference)+(c.probabilityPpm!==undefined?` (attributed probability ${c.probabilityPpm/10000}%)`:c.scorePpm!==undefined?` (uncalibrated score ${c.scorePpm/1000000})`:''));
      const excluded=set.excluded.map(e=>`${e.value}${reference(e.reference)}, because ${e.reason}; coverage: ${e.coverage}`);
      const same=(a:EquipmentAlternative|EquipmentExclusion,b:EquipmentAlternative|EquipmentExclusion)=>{
        if(a.reference&&b.reference)return a.reference.issuer===b.reference.issuer&&a.reference.key===b.reference.key&&a.reference.basis===b.reference.basis;
        return a.reference===undefined&&b.reference===undefined&&a.value===b.value;
      };
      const conflict=set.candidates.some(c=>set.excluded.some(e=>same(c,e)));
      value=[possible.length?'Possible: '+possible.join(', '):'',excluded.length?'Excluded: '+excluded.join('; '):'',set.exhaustive?'Declared complete domain':`Incomplete alternatives; other ${field.name.replaceAll('_',' ')} values remain unknown`,set.residualPpm!==undefined?`Unknown probability: ${set.residualPpm/10000}%`:'',set.domain?`Domain: ${set.domain}`:'',set.calibrationBasis?`Attributed calibration: ${set.calibrationBasis}`:'',conflict?'Conflicting claims require review':''].filter(Boolean).join(' · ');
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
