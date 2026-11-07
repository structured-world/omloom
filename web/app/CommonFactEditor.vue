<script setup lang="ts">
import { computed, nextTick, onMounted, ref } from 'vue';
import { call, display, evaluateFieldGroup, key, model, requestMessage, typedField, type AssetRef, type Field } from './api';
import EquipmentField from './EquipmentField.vue';

const props = defineProps<{
  field: string;
  members: { condition: model.FieldRequirement; label: string }[];
  working: model.WorkingContext;
}>();
const emit = defineEmits<{ close: []; busy: [boolean]; saved: [targets: AssetRef[]]; accessChanged: []; unconfirmed: [targets: AssetRef[]] }>();
const included = ref(new Set(props.members.map(member => key(member.condition.target))));
const selected = computed(() => props.members.filter(member => included.value.has(key(member.condition.target))));
const descriptionMode = ref<'preserve' | 'set' | 'clear'>('preserve');
const description = ref('');
const equipment = ref<Field>();
const valid = ref(true);
const proposed = computed(() => props.field === 'description'
  ? descriptionMode.value === 'preserve' ? undefined : typedField(props.field, model.FieldType.TEXT, description.value, descriptionMode.value === 'clear')
  : valid.value ? equipment.value : undefined);
const busy = ref(false);
const error = ref('');
const uncertain = ref(false);
const inaccessible = ref(new Set<string>());
type ReviewRow = { id: string; before?: Field; source?: string; allowed: boolean };
const preview = ref<{ revision: bigint; rows: ReviewRow[]; reviewedDraft: boolean }>();
const canApply = computed(() => !!preview.value?.reviewedDraft && preview.value.rows.every(row => row.allowed));
const after = computed(() => display(proposed.value));
const heading = ref<HTMLElement>();
const reviewHeading = ref<HTMLElement>();
let preferredAnchor: string | undefined;
onMounted(() => heading.value?.focus());

function invalidate() {
  preview.value = undefined;
  error.value = '';
}
function include(id: string, value: boolean) {
  if (value) included.value.add(id);
  else included.value.delete(id);
  invalidate();
}
function setBusy(value: boolean) { busy.value = value; emit('busy', value); }
async function review(event: SubmitEvent) {
  const reviewedDraft = !!proposed.value && event.currentTarget instanceof HTMLFormElement && event.currentTarget.matches(':valid');
  if (busy.value || (!uncertain.value && !reviewedDraft) || !selected.value.length) return;
  invalidate();
  const fields = selected.value.map(member => member.condition);
  setBusy(true);
  let anchorDenied = false;
  try {
    const { reply, anchor } = await evaluateFieldGroup(fields, props.working, {
      preferredAnchor,
      onAccessFailure() {
        if (anchorDenied) return;
        anchorDenied = true;
        for (const field of fields) inaccessible.value.add(key(field.target));
        emit('accessChanged');
      },
    });
    if (reply.oneofKind !== 'requirementEvaluation') throw Error('Current facts are unavailable.');
    preferredAnchor = anchor;
    const { revision, results } = reply.requirementEvaluation;
    const byNode = new Map(results.map(result => [result.node, result]));
    const rows = fields.map((condition, index) => {
      const id = key(condition.target);
      const result = byNode.get(index);
      const readable = result && key(result.target) === id && result.field === props.field
        && result.reason !== model.RequirementReason.RESTRICTED;
      if (readable) inaccessible.value.delete(id);
      else inaccessible.value.add(id);
      return { id, before: readable ? result.evidence[0]?.value : undefined,
        source: readable ? result.evidence[0]?.source : undefined,
        allowed: !!readable && result.fieldWritable };
    });
    if (rows.some(row => inaccessible.value.has(row.id))) emit('accessChanged');
    preview.value = { revision, rows, reviewedDraft };
    await nextTick();
    reviewHeading.value?.focus();
  } catch (failure) {
    error.value = requestMessage(failure);
  } finally { setBusy(false); }
}
async function apply() {
  const check = preview.value; const change = proposed.value;
  if (busy.value || !check || !canApply.value || !change) return;
  const targets = selected.value.flatMap(member => member.condition.target ? [member.condition.target] : []);
  setBusy(true);
  try {
    const reply = await call({ oneofKind: 'bulk', bulk: model.Bulk.create({
      targets, expected: check.revision, changes: [change], sharedFact: true,
    }) }, { ...props.working, expected: check.revision });
    if (reply.oneofKind !== 'mutation') throw Error('The save result is unavailable.');
    emit('saved', targets);
  } catch (failure) {
    invalidate();
    const code = failure instanceof Error && 'code' in failure ? failure.code : undefined;
    if (!['ABORTED', 'PERMISSION_DENIED', 'NOT_FOUND', 'INVALID_ARGUMENT', 'RESOURCE_EXHAUSTED', 'FAILED_PRECONDITION'].includes(String(code))) {
      uncertain.value = true;
      emit('unconfirmed', targets);
    }
    if (code === 'PERMISSION_DENIED' || code === 'NOT_FOUND') {
      for (const target of targets) inaccessible.value.add(key(target));
      emit('accessChanged');
    }
    error.value = uncertain.value ? 'Save not confirmed. Review current values before applying again.' : requestMessage(failure);
  } finally { setBusy(false); }
}
function before(row: ReviewRow) { return row.before ? display(row.before) : 'Unknown'; }
</script>

<template>
<section class="selected-requirements shared-fact-review" aria-label="Shared fact">
  <h3 ref="heading" tabindex="-1">Apply one value for {{ field.replaceAll('_', ' ') }}</h3>
  <p>Use a value and basis that apply to every included device. The batch updates accepted facts together.</p>
  <form @submit.prevent="review">
    <fieldset class="shared-fact-members" :disabled="busy">
      <legend>Included devices</legend>
      <label v-for="(member, index) in members" :key="key(member.condition.target)">
        <input type="checkbox" :checked="included.has(key(member.condition.target))" @change="include(key(member.condition.target), ($event.target as HTMLInputElement).checked)" />
        {{ inaccessible.has(key(member.condition.target)) ? `Device ${index + 1} (unavailable)` : member.label }}
      </label>
    </fieldset>
    <fieldset :disabled="busy">
      <legend>New accepted value</legend>
      <template v-if="field === 'description'">
        <label>Change<select v-model="descriptionMode" @change="invalidate">
          <option value="preserve">Choose a change</option><option value="set">Set description</option><option value="clear">Clear explicitly</option>
        </select></label>
        <label v-if="descriptionMode === 'set'">Description <input v-model="description" maxlength="2048" @input="invalidate" /></label>
      </template>
      <EquipmentField v-else :name="field" focused allow-clear @change="equipment = $event; invalidate()" @valid="valid = $event; invalidate()" />
    </fieldset>
    <p v-if="error" role="alert">{{ error }}</p>
    <p v-if="uncertain && preview" role="status">These are the current values. They cannot confirm who made the previous change.</p>
    <button type="submit" :formnovalidate="uncertain" :disabled="busy || (!uncertain && !proposed) || !selected.length">{{ uncertain ? 'Review current values' : 'Review shared change' }}</button>
    <template v-if="preview">
      <h4 ref="reviewHeading" tabindex="-1">Review {{ preview.rows.length }} included devices</h4>
      <ol class="fact-review-list">
        <li v-for="(row, index) in preview.rows" :key="row.id" class="fact-review-row">
          <h4>{{ inaccessible.has(row.id) ? 'Unavailable device fact' : selected[index]?.label }}</h4>
          <p v-if="!row.allowed">This device cannot be changed in this batch. Exclude it explicitly to review a narrower batch.</p>
          <template v-else>
            <div class="fact-current"><p>Before: {{ before(row) }}<small v-if="row.source">Accepted authority: {{ row.source }}</small></p></div>
            <div v-if="preview.reviewedDraft" class="fact-current"><p>After: {{ after }}</p></div>
          </template>
        </li>
      </ol>
      <button class="primary" type="button" :disabled="busy || !canApply" @click="apply">Apply to reviewed devices</button>
    </template>
    <button type="button" :disabled="busy" @click="emit('close')">Back to checks</button>
  </form>
</section>
</template>
