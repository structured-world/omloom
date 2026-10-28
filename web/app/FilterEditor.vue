<script setup lang="ts">
import { ref, computed } from 'vue';
import Dialog from './Dialog.vue';
import { softwareFields, softwareChoiceLabel, softwareChoices } from './software-fields';
import { model, typedField } from './api';
const props = defineProps<{ definitions: model.Definition[]; predicates: model.Predicate[]; responsibility?:model.ResponsibilityFilter }>();
const emit = defineEmits<{ close: []; apply: [model.Predicate[],model.ResponsibilityFilter?] }>();
const ownerFilter=ref(false);const facet=ref(model.ResponsibilityFacet.CLAIMED_OWNER);const issuer=ref(props.responsibility?.issuer ?? '');const subject=ref(props.responsibility?.subject ?? '');
const selected = ref('name'); const value = ref(''); const cleared = ref(false); const error = ref('');
const fields = computed(() => [{ id: 'name', name: 'Name', type: model.FieldType.TEXT }, { id: 'description', name: 'Description', type: model.FieldType.TEXT }, { id: 'serial', name: 'Serial number', type: model.FieldType.TEXT }, ...softwareFields(), ...props.definitions.filter(d => !d.retired)]);
const definition = computed(() => fields.value.find(d => d.id === selected.value));
const choices = computed(() => softwareChoices(selected.value));
function apply() {
  try {
    if(ownerFilter.value){emit('apply',props.predicates,model.ResponsibilityFilter.create({facet:facet.value,issuer:issuer.value,subject:subject.value}));return;}
    if (!definition.value) throw Error('Select an available field');
    if (!cleared.value && choices.value.length && !choices.value.includes(value.value)) throw Error('Choose an available value.');
    const field = typedField(selected.value, definition.value.type, value.value, cleared.value);
    const d = props.definitions.find(d => d.id === selected.value);
    emit('apply', [...props.predicates, model.Predicate.create({ field: selected.value, value: field, definitionGeneration: d?.generation })],props.responsibility);
  } catch (e) { error.value = String(e); }
}
</script>
<template><Dialog title="Add typed filter" @close="emit('close')"><form @submit.prevent="apply">
<label class="check"><input v-model="ownerFilter" type="checkbox">Filter by effective responsibility</label>
<template v-if="ownerFilter"><label for="filter-responsibility-facet">Responsibility facet</label><select id="filter-responsibility-facet" v-model.number="facet"><option :value="model.ResponsibilityFacet.CLAIMED_OWNER">Claimed owner</option><option :value="model.ResponsibilityFacet.OPERATOR">Operator</option><option :value="model.ResponsibilityFacet.BENEFICIARY">Beneficiary</option><option :value="model.ResponsibilityFacet.ACCOUNT_CONTROLLER">Account controller</option></select><label>Party namespace<input v-model="issuer" required maxlength="256"></label><label>Party identity<input v-model="subject" required maxlength="256"></label><p class="hint">Includes permitted inherited assignments before counting and pagination. A party name is not its identity.</p></template>
<template v-else><label for="filter-field">Field</label><select id="filter-field" v-model="selected"><option v-for="f in fields" :key="f.id" :value="f.id">{{ f.name }}</option></select><label class="check"><input v-model="cleared" type="checkbox">Matches explicit clear, not missing values</label><label v-if="!cleared">Equals<select v-if="choices.length" v-model="value" required><option disabled value="">Choose a value</option><option v-for="choice in choices" :key="choice" :value="choice">{{ softwareChoiceLabel(selected, choice) }}</option></select><input v-else v-model="value" :type="definition?.type === model.FieldType.DATE ? 'date' : 'text'" :inputmode="definition?.type === model.FieldType.INTEGER ? 'numeric' : undefined" required maxlength="2048"></label></template>
<p class="hint">Definitions and their revisions remain part of this filter. An unavailable or changed field is an error; it never silently broadens the results.</p><p v-if="error" role="alert">{{ error }}</p><footer><button type="button" @click="emit('close')">Cancel</button><button v-if="responsibility" type="button" @click="emit('apply',predicates)">Remove responsibility filter</button><button class="primary">Apply filter</button></footer></form></Dialog></template>
