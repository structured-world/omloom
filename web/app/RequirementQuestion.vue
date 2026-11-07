<script setup lang="ts">
import { computed, ref } from 'vue';
import { model, AssetKind } from './api';
import { equipmentFields } from './equipment-fields';
import type { Field } from '../src/generated/omloom/v1/contracts';
const props = defineProps<{ kind: AssetKind; busy: boolean; disabled?: boolean; plan?: boolean }>();
const emit = defineEmits<{ check: [condition: Omit<model.FieldRequirement, 'target'>, label: string]; change: [] }>();
const field = ref(''); const value = ref(''); const comparison = ref(model.RequirementComparison.EQUAL);
const layer = ref(model.RequirementLayer.ACCEPTED); const error = ref('');
const checkButton = ref<HTMLButtonElement>();
defineExpose({ focusCheck: () => checkButton.value?.focus() });
const units: Record<string, string> = { drive_bays: 'count', height: 'mm', width: 'mm', depth: 'mm', weight: 'g', power_rating: 'W' };
const numeric = computed(() => field.value in units);
const needsValue = computed(() => field.value !== 'description');
const fields = computed(() => props.kind === AssetKind.PROVIDER_ACCOUNT ? ['delegation'] : [...equipmentFields, 'description']);
function changed() {
  error.value = ''; emit('change');
  if (!numeric.value) comparison.value = model.RequirementComparison.EQUAL;
  else if (field.value !== 'drive_bays' && comparison.value === model.RequirementComparison.EQUAL) comparison.value = model.RequirementComparison.UNSPECIFIED;
}
function check() {
  error.value = '';
  if (!field.value || (needsValue.value && !value.value)) return;
  try {
    let expected: Field['value'];
    if (numeric.value) {
      if (!/^\d+(\.\d{1,6})?$/.test(value.value)) throw Error('Enter a nonnegative measurement with at most six decimal places.');
      const scale = value.value.includes('.') ? value.value.length - value.value.indexOf('.') - 1 : 0;
      if (field.value === 'drive_bays' && scale) throw Error('Enter a whole number of bays.');
      const fixed = BigInt(value.value.replace('.', ''));
      if (fixed > 9223372036854775807n) throw Error('The measurement exceeds the supported integer range.');
      if (field.value === 'drive_bays') expected = { oneofKind: 'integer', integer: fixed };
      else {
        if (comparison.value === model.RequirementComparison.EQUAL) throw Error('Select at least or at most for a measurement.');
        expected = { oneofKind: 'equipment', equipment: { basis: 'Requested constraint', assertion: { oneofKind: 'quantity', quantity: { lower: fixed, upper: fixed, unit: units[field.value]!, scale } } } };
      }
    } else expected = { oneofKind: 'text', text: value.value };
    const label = `${field.value.replaceAll('_', ' ')} ${comparison.value === model.RequirementComparison.MINIMUM ? 'at least' : comparison.value === model.RequirementComparison.MAXIMUM ? 'at most' : 'equals'} ${value.value}${numeric.value ? ' ' + units[field.value] : ''} (${layer.value === model.RequirementLayer.ACCEPTED ? 'accepted configuration' : 'dated observations'})`;
    emit('check', { expected: { name: field.value, value: expected }, comparison: comparison.value, layer: layer.value }, label);
  } catch (e) { error.value = String(e); }
}
</script>
<template>
<form class="requirement-question" @submit.prevent="check" @input="changed" @change="changed">
<fieldset :disabled="busy || disabled">
<label>Required fact<select v-model="field" required><option disabled value="">Choose a fact</option><option v-for="name in fields" :key="name" :value="name">{{ name.replaceAll('_', ' ') }}</option></select></label>
<label>Evidence layer<select v-model="layer"><option :value="model.RequirementLayer.ACCEPTED">Accepted configuration</option><option :value="model.RequirementLayer.OBSERVED">Dated observations</option></select></label>
<label>Required comparison<select v-model="comparison"><option disabled :value="model.RequirementComparison.UNSPECIFIED">Choose a comparison</option><option v-if="!numeric || field === 'drive_bays'" :value="model.RequirementComparison.EQUAL">Equals</option><option v-if="numeric" :value="model.RequirementComparison.MINIMUM">At least</option><option v-if="numeric" :value="model.RequirementComparison.MAXIMUM">At most</option></select></label>
<label>Required value<span v-if="numeric"> ({{ units[field] }})</span><select v-if="field === 'delegation'" v-model="value" required><option disabled value="">Select capability</option><option value="ACCOUNT_DELEGATION_SCOPED">Scoped delegation</option><option value="ACCOUNT_DELEGATION_OWNER_ONLY">Owner action</option></select><input v-else v-model="value" :inputmode="numeric ? 'decimal' : 'text'" maxlength="160" :required="needsValue" /></label>
<button ref="checkButton" type="submit" :disabled="!field || (needsValue && !value) || comparison === model.RequirementComparison.UNSPECIFIED">{{ busy ? 'Checking…' : plan ? 'Use this postcondition' : 'Check requirement' }}</button>
</fieldset>
<p v-if="error" role="alert">{{ error }}</p>
</form>
</template>
