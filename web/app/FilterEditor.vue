<script setup lang="ts">
import { ref, computed } from 'vue';
import Dialog from './Dialog.vue';
import { model, text, type Field } from './api';
const props = defineProps<{ definitions: model.Definition[]; predicates: model.Predicate[] }>();
const emit = defineEmits<{ close: []; apply: [model.Predicate[]] }>();
const selected = ref('name'); const value = ref(''); const cleared = ref(false); const error = ref('');
const fields = computed(() => [{ id: 'name', name: 'Name', type: model.FieldType.TEXT }, { id: 'description', name: 'Description', type: model.FieldType.TEXT }, { id: 'serial', name: 'Serial number', type: model.FieldType.TEXT }, ...props.definitions.filter(d => !d.retired)]);
const definition = computed(() => fields.value.find(d => d.id === selected.value));
function apply() {
  try {
    let field: Field = text(selected.value, value.value);
    if (cleared.value) field = { name: selected.value, value: { oneofKind: 'clear', clear: 0 } };
    else if (definition.value?.type === model.FieldType.INTEGER) field = { name: selected.value, value: { oneofKind: 'integer', integer: BigInt(value.value) } };
    else if (definition.value?.type === model.FieldType.BOOLEAN) {
      if (!['true', 'false'].includes(value.value)) throw Error('Select true or false');
      field = { name: selected.value, value: { oneofKind: 'flag', flag: value.value === 'true' } };
    }
    const d = props.definitions.find(d => d.id === selected.value);
    emit('apply', [...props.predicates, model.Predicate.create({ field: selected.value, value: field, definitionRevision: d?.revision })]);
  } catch (e) { error.value = String(e); }
}
</script>
<template><Dialog title="Add typed filter" @close="emit('close')"><form @submit.prevent="apply"><label for="filter-field">Field</label><select id="filter-field" v-model="selected"><option v-for="f in fields" :key="f.id" :value="f.id">{{ f.name }}</option></select><label class="check"><input v-model="cleared" type="checkbox">Matches explicit clear, not missing values</label><label v-if="!cleared">Equals<input v-model="value" :type="definition?.type === model.FieldType.INTEGER ? 'number' : definition?.type === model.FieldType.DATE ? 'date' : 'text'" required maxlength="2048"></label><p class="hint">Definitions and their revisions remain part of this filter. An unavailable or changed field is an error; it never silently broadens the results.</p><p v-if="error" role="alert">{{ error }}</p><footer><button type="button" @click="emit('close')">Cancel</button><button class="primary">Apply filter</button></footer></form></Dialog></template>
