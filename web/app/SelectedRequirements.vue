<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref } from 'vue';
import { call, model, key, display, resourceName, enumLabel, AssetKind, type AssetRef } from './api';
import Dialog from './Dialog.vue';
import RequirementQuestion from './RequirementQuestion.vue';
import AssetEditor from './AssetEditor.vue';
import ImportEditor from './ImportEditor.vue';
const props = defineProps<{ items: model.Item[]; working: model.WorkingContext; definitions: model.Definition[]; findings?: model.FieldRequirement[] }>();
const emit = defineEmits<{ close: []; saved: [] }>();
// The reviewed population belongs to this view, independently of the list's filters and refreshes.
const items = props.items.map(item => model.Item.create(item));
const working = model.WorkingContext.create({ ...props.working, expected: undefined });
const conditions = ref<model.FieldRequirement[]>([]);
type Answer = { result: model.RequirementResult; revision: bigint; evaluatedAtMs: bigint };
const answers = ref(new Map<string, Answer>());
const labels = ref(new Map<string, string>());
const error = ref(''); const busy = ref(false); const importing = ref(false); const adding = ref(true);
const notice = ref(''); const revision = ref<bigint>();
type Draft = { id: string; item: model.Item; field: string; question: string };
const drafts = ref<Draft[]>([]); const active = ref<string>(); const writing = ref(false);
const draftIds = computed(() => new Set(drafts.value.map(draft => draft.id)));
const deniedFields = ref(new Set<string>());
const suspended = ref(false);
const heading = ref<HTMLElement>();
let generation = 0; let disposed = false; let reviewAnchor: string | undefined;
onBeforeUnmount(() => { disposed = true; generation++; });
function identity(condition: model.FieldRequirement) { return model.FieldRequirement.toJsonString(condition); }
function fieldKey(target: AssetRef | undefined, field: string | undefined) { return `${key(target)}:${field}`; }
function factKey(condition: model.FieldRequirement) { return `${fieldKey(condition.target, condition.expected?.name)}:${condition.layer}`; }
function questionLabel(condition: model.FieldRequirement) {
  const expected = condition.expected;
  return `${expected?.name.replaceAll('_', ' ')} ${enumLabel(model.RequirementComparison, condition.comparison).toLowerCase()} ${display(expected)} (${enumLabel(model.RequirementLayer, condition.layer).toLowerCase()})`;
}
const rows = computed(() => {
  const result = new Map<string, { id: string; target: AssetRef; item: model.Item; field: string; conditions: model.FieldRequirement[] }>();
  for (const condition of conditions.value) {
    const item = items.find(i => key(i.target) === key(condition.target));
    if (!item?.target || !condition.expected) continue;
    const id = factKey(condition);
    let row = result.get(id);
    if (!row) { row = { id, target: item.target, item, field: condition.expected.name, conditions: [] }; result.set(id, row); }
    row.conditions.push(condition);
  }
  return [...result.values()];
});
function result(condition: model.FieldRequirement) { return answers.value.get(identity(condition))?.result; }
function checkedAt(condition: model.FieldRequirement) {
  const answer = answers.value.get(identity(condition));
  if (answer) return `revision ${answer.revision}, ${new Date(Number(answer.evaluatedAtMs)).toLocaleString()}`;
}
function editable(row: typeof rows.value[number]) {
  return !suspended.value && !restricted(row) && (draftIds.value.has(row.id) || row.conditions.some(c => result(c)?.resolutions.includes(model.RequirementResolution.RECORD_FACT)));
}
function current(row: typeof rows.value[number]) { return result(row.conditions[0]!)?.evidence ?? []; }
function restricted(row: typeof rows.value[number]) { return deniedFields.value.has(fieldKey(row.target, row.field)); }
function discardRestrictedDrafts() {
  const denied = new Set(rows.value.filter(restricted).map(row => row.id));
  if (denied.size) notice.value = '';
  drafts.value = drafts.value.filter(draft => !denied.has(draft.id));
  if (active.value && denied.has(active.value)) active.value = undefined;
}
function mismatch(row: typeof rows.value[number]) { return row.conditions.some(c => result(c)?.reason === model.RequirementReason.MISMATCH); }
function name(row: typeof rows.value[number]) { return resourceName(row.item) || `Device (${row.target.assetId.slice(-8)})`; }
function accessFailure(error: unknown) {
  return error instanceof Error && 'code' in error && (error.code === 'PERMISSION_DENIED' || error.code === 'NOT_FOUND');
}
function suspendReview() {
  suspended.value = true;
  answers.value = new Map();
  notice.value = '';
  revision.value = undefined;
}
async function evaluateFromSelection(query: model.EvaluateRequirements, selected: model.FieldRequirement[], fullReview: boolean, turn: number) {
  const anchors = fullReview ? new Map(selected.map(condition => [key(condition.target), condition.target])) : new Map();
  const first = reviewAnchor ? anchors.get(reviewAnchor) ?? query.anchor : query.anchor;
  anchors.delete(key(first));
  const candidates = [first, ...anchors.values()];
  for (const [index, anchor] of candidates.entries()) {
    query.anchor = anchor;
    try {
      const reply = await call({ oneofKind: 'evaluateRequirements', evaluateRequirements: query }, working);
      if (disposed || turn !== generation) return;
      if (fullReview && reply.oneofKind === 'requirementEvaluation') reviewAnchor = key(anchor);
      return reply;
    } catch (error) {
      if (disposed || turn !== generation) return;
      if (!accessFailure(error)) throw error;
      // An unavailable anchor and an invalid context both fail before leaf
      // results. Keep evidence hidden and let the server admit each retry.
      if (!suspended.value) suspendReview();
      if (index === candidates.length - 1) throw error;
    }
  }
}
async function evaluate(selected: model.FieldRequirement[]) {
  if (suspended.value) selected = conditions.value;
  if (!selected.length) return;
  const fullReview = selected.length === conditions.value.length;
  const turn = ++generation; busy.value = true; error.value = '';
  // Invalidate affected answers first, so an old success cannot survive an unsuccessful refresh.
  const next = new Map(answers.value); for (const condition of selected) next.delete(identity(condition)); answers.value = next;
  try {
    const query = model.EvaluateRequirements.create({ anchor: selected[0]!.target, root: selected.length, nodes: [
      ...selected.map(field => ({ expression: { oneofKind: 'field' as const, field } })),
      { expression: { oneofKind: 'group', group: { kind: model.RequirementGroupKind.ALL, minimum: 0, members: selected.map((_, i) => i) } } },
    ] });
    const reply = await evaluateFromSelection(query, selected, fullReview, turn);
    if (disposed || turn !== generation) return;
    if (reply?.oneofKind !== 'requirementEvaluation') throw Error('The selected checks are unavailable.');
    const updated = new Map(answers.value);
    const checkedFields = new Set<string>(); const restrictedFields = new Set<string>();
    for (const entry of reply.requirementEvaluation.results) {
      const condition = selected[entry.node];
      if (!condition) continue;
      const field = fieldKey(condition.target, condition.expected?.name);
      checkedFields.add(field);
      if (entry.reason === model.RequirementReason.RESTRICTED) restrictedFields.add(field);
      else updated.set(identity(condition), { result: entry, revision: reply.requirementEvaluation.revision, evaluatedAtMs: reply.requirementEvaluation.evaluatedAtMs });
    }
    // Field-read authority is shared by accepted and observed evidence.
    const denied = new Set(deniedFields.value);
    for (const field of checkedFields) denied.delete(field);
    for (const field of restrictedFields) denied.add(field);
    if (denied.size) {
      for (const condition of conditions.value) {
        if (denied.has(fieldKey(condition.target, condition.expected?.name))) updated.delete(identity(condition));
      }
    }
    deniedFields.value = denied;
    answers.value = updated; revision.value = reply.requirementEvaluation.revision;
    // Revocation removes protected values and retained drafts, not just their edit buttons.
    discardRestrictedDrafts();
    suspended.value = false;
  } catch (e) { if (!disposed && turn === generation) {
    error.value = String(e);
    if (accessFailure(e)) {
      if (!fullReview) {
        await evaluate(conditions.value);
        return;
      }
      deniedFields.value = new Set(selected.map(condition => fieldKey(condition.target, condition.expected?.name)));
      discardRestrictedDrafts();
      suspended.value = false;
    }
  } }
  finally { if (!disposed && turn === generation) busy.value = false; }
}
async function check(condition: Omit<model.FieldRequirement, 'target'>, label: string) {
  if (conditions.value.length + items.length > 63) { error.value = 'Keep at most 63 conditions in this review. Split the selection into explicit batches.'; return; }
  const added = items.flatMap(item => item.target ? [model.FieldRequirement.create({ ...condition, target: item.target })] : []);
  const unique = added.filter(c => !conditions.value.some(old => identity(old) === identity(c)));
  conditions.value.push(...unique); const next = new Map(labels.value); for (const c of unique) next.set(identity(c), label); labels.value = next;
  adding.value = false; await evaluate(unique);
}
async function edit(row: typeof rows.value[number]) {
  if (writing.value || busy.value || !editable(row)) return;
  if (draftIds.value.has(row.id)) { active.value = row.id; return; }
  const turn = ++generation; busy.value = true; error.value = '';
  try {
    const reply = await call({ oneofKind: 'read', read: model.Read.create({ target: row.target, acceptedField: row.field }) }, working);
    if (disposed || turn !== generation) return;
    if (reply.oneofKind !== 'detail' || key(reply.detail.item?.target) !== key(row.target) || !reply.detail.item) throw Error('This fact is unavailable.');
    drafts.value.push({ id: row.id, item: reply.detail.item, field: row.field, question: row.conditions.map(c => labels.value.get(identity(c)) ?? questionLabel(c)).join('; ') });
    active.value = row.id;
  } catch (e) { if (!disposed && turn === generation) { error.value = String(e); await evaluate(row.conditions); } }
  finally { if (!disposed && turn === generation) busy.value = false; }
}
async function focusRow(id?: string) {
  await nextTick();
  const button = id ? document.getElementById(`fact-${id}`) : undefined;
  (button ?? heading.value)?.focus();
}
async function finish(id: string, saved: boolean) {
  const index = rows.value.findIndex(row => row.id === id); const row = rows.value[index]; if (!row) return;
  active.value = undefined; drafts.value = drafts.value.filter(d => d.id !== id);
  if (saved) { notice.value = `Saved ${row.field.replaceAll('_', ' ')} for ${name(row)}. Checking the original conditions.`; emit('saved'); }
  // The same accepted edit can affect several layers, but cannot refresh source observations.
  await evaluate(conditions.value.filter(c => key(c.target) === key(row.target) && c.expected?.name === row.field));
  const next = rows.value.slice(index + 1).find(candidate => !draftIds.value.has(candidate.id) && editable(candidate));
  await focusRow(next?.id);
  if (next) await edit(next);
}
async function defer() {
  const id = active.value; if (!id || writing.value) return;
  active.value = undefined;
  const index = rows.value.findIndex(row => row.id === id);
  const next = rows.value.slice(index + 1).find(row => !draftIds.value.has(row.id) && editable(row));
  await focusRow(next?.id); if (next) await edit(next);
}
async function closeEditor(id: string) {
  drafts.value = drafts.value.filter(d => d.id !== id); active.value = undefined; await focusRow(id);
}
async function recheck(id: string, unresolved: boolean) {
  const row = rows.value.find(row => row.id === id);
  if (unresolved) { if (row) await evaluate(row.conditions); }
  else await finish(id, false);
}
async function imported() { importing.value = false; emit('saved'); notice.value = 'Import accepted. Rechecking the original selection and conditions.'; await evaluate(conditions.value); await focusRow(); }
onMounted(async () => {
  if (!items.length || items.length > 63 || items.some(item => item.kind !== AssetKind.DEVICE || !item.target)) { error.value = 'Select between one and 63 devices.'; return; }
  if (props.findings) {
    if (props.findings.length > 63 || props.findings.some(c => !items.some(item => key(item.target) === key(c.target)) || !c.expected)) { error.value = 'Findings must concern this explicit device selection.'; return; }
    conditions.value = [...new Map(props.findings.map(c => [identity(c), model.FieldRequirement.create(c)])).values()];
    adding.value = false; await evaluate(conditions.value);
  }
});
</script>
<template>
<Dialog title="Check selected devices" :dismissible="!writing" @close="emit('close')">
<section class="selected-requirements">
<h3 ref="heading" tabindex="-1">{{ items.length }} selected devices</h3>
<p>The selection stays fixed in this review. Missing facts may remain unresolved. Correct a value only when the recorded fact is wrong.</p>
<RequirementQuestion v-if="adding" :kind="AssetKind.DEVICE" :busy="busy" :disabled="suspended || writing || !!active" @check="check" />
<div class="row"><button v-if="conditions.length && !adding" :disabled="suspended || busy || writing || !!active" @click="adding = true">Add another condition</button><button v-if="conditions.length" :disabled="busy || writing" @click="evaluate(conditions)">Refresh checks</button><button :disabled="suspended || busy || writing || !!active" @click="importing = true">Import CSV and return</button></div>
<p v-if="notice" role="status">{{ notice }}</p><p v-if="error" role="alert">{{ error }}</p><p v-if="busy" role="status">Checking selected facts…</p>
<p v-if="revision !== undefined">Last check used inventory revision {{ revision.toString() }}. Results are not saved.</p>
<p v-if="suspended && !busy" role="status">Access to this review could not be verified. Refresh checks to continue.</p>
<ol v-if="!suspended" class="fact-review-list">
<li v-for="row in rows" :key="row.id" class="fact-review-row" :class="{ 'is-current': active === row.id }">
<h4>{{ restricted(row) ? 'Unavailable device fact' : name(row) }} <small v-if="!restricted(row)">{{ row.field.replaceAll('_', ' ') }}</small></h4>
<template v-if="restricted(row)"><p>This fact is unavailable.</p></template>
<template v-else>
<div class="fact-current"><strong>Currently recorded:</strong><span v-if="!current(row).length">{{ result(row.conditions[0]!) ? 'Unknown' : 'Not checked' }}</span><p v-for="(evidence, i) in current(row)" :key="i">{{ evidence.value ? display(evidence.value) : 'Value unavailable' }}<small>{{ evidence.observedAtMs === undefined ? 'Accepted authority' : 'Observation source' }}: {{ evidence.source }}<template v-if="evidence.validUntilMs !== undefined"> · valid until {{ new Date(Number(evidence.validUntilMs)).toLocaleString() }}</template></small></p></div>
<ul class="fact-conditions"><li v-for="condition in row.conditions" :key="identity(condition)"><span>{{ labels.get(identity(condition)) ?? questionLabel(condition) }}</span><strong>{{ result(condition) ? enumLabel(model.RequirementState, result(condition)!.state).toLowerCase() : 'Not checked' }}</strong><small v-if="result(condition)">{{ enumLabel(model.RequirementReason, result(condition)!.reason).toLowerCase() }} · {{ checkedAt(condition) }}</small></li></ul>
<button v-if="editable(row)" :id="`fact-${row.id}`" :disabled="busy || writing || (!!active && active !== row.id)" @click="edit(row)">{{ draftIds.has(row.id) ? 'Resume saved input' : mismatch(row) ? 'Correct recorded fact' : 'Clarify this fact' }}</button>
<p v-if="draftIds.has(row.id) && active !== row.id">Deferred, input retained.</p>
</template>
</li>
</ol>
<template v-for="draft in drafts" :key="draft.id">
<section v-show="!suspended && active === draft.id" class="fact-review-editor" aria-label="Selected fact editor">
<AssetEditor inline :inactive="suspended || active !== draft.id" :item="draft.item" :definitions="definitions" :field-mask="[draft.field]" :question="draft.question" :working="{ ...working, focus: draft.item.target }" :view="model.View.HARDWARE" save-next @pending="writing = $event" @saved="finish(draft.id, true)" @recheck="recheck(draft.id, $event)" @close="closeEditor(draft.id)" />
<button type="button" :disabled="writing || busy" @click="defer">Defer this row and continue</button>
</section>
</template>
</section>
<ImportEditor v-if="importing" :definitions="definitions" :working="working" @close="importing = false" @saved="imported" />
</Dialog>
</template>
