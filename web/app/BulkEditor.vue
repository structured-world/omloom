<script setup lang="ts">
import { computed, ref } from 'vue';
import Dialog from './Dialog.vue';
import { call, model, typedField, display, key, AssetKind } from './api';
const props = defineProps<{ items: model.Item[]; revision: bigint; definitions: model.Definition[]; working: model.WorkingContext }>();
const emit = defineEmits<{ close: []; saved: [] }>();
const name = ref('description'); const mode = ref('preserve'); const value = ref(''); const error = ref('');
const fields = computed(() => [{ id: 'description', name: 'Description', type: model.FieldType.TEXT }, { id: 'serial', name: 'Serial number', type: model.FieldType.TEXT }, ...(props.items.length && props.items.every(i=>i.kind===AssetKind.DOMAIN) ? [{id:'dns_name',name:'DNS name',type:model.FieldType.TEXT}] : []), ...props.definitions.filter(d => !d.retired && props.items.every(i => d.kinds.includes(i.kind)))]);
const field = computed(() => fields.value.find(f => f.id === name.value));
async function apply(remove = false) {
  try {
    if (!field.value) throw Error('Select an applicable field');
    const changes = remove || mode.value === 'preserve' ? [] : [typedField(name.value, field.value.type, value.value, mode.value === 'clear')];
    await call({ oneofKind: 'bulk', bulk: { targets: props.items.flatMap(i => i.target ? [i.target] : []), expected: props.revision, remove, changes } }, props.working); emit('saved');
  } catch (e) { error.value = String(e); }
}
</script>
<template><Dialog title="Review bulk change" @close="emit('close')"><form @submit.prevent="apply()">
  <p>Apply only to these {{ items.length }} reviewed identities, at revision {{ revision }}. Omitted fields are preserved.</p>
  <ul><li v-for="i in items" :key="key(i.target)">{{ display(i.fields.find(f => f.name === 'name')) }} · {{ i.target?.assetId }}</li></ul>
  <label>Field<select v-model="name"><option v-for="f in fields" :key="f.id" :value="f.id">{{ f.name }}</option></select></label>
  <label>Change<select v-model="mode"><option value="preserve">Preserve</option><option value="set">Set</option><option value="clear">Clear explicitly</option></select></label>
  <label v-if="mode === 'set'">Value
    <select v-if="field?.type === model.FieldType.BOOLEAN" v-model="value" required><option disabled value="">Choose true or false</option><option value="true">True</option><option value="false">False</option></select>
    <select v-else-if="field && 'choices' in field && field.choices.length" v-model="value" required><option disabled value="">Choose a value</option><option v-for="choice in field.choices" :key="choice">{{ choice }}</option></select>
    <input v-else v-model="value" :type="field?.type === model.FieldType.DATE ? 'date' : 'text'" :inputmode="field?.type === model.FieldType.INTEGER ? 'numeric' : undefined" maxlength="2048">
  </label>
  <p v-if="error" role="alert">{{ error }}</p><footer><button type="button" @click="emit('close')">Cancel</button><button class="danger" type="button" @click="apply(true)">Remove reviewed resources</button><button class="primary" :disabled="mode === 'preserve'">Apply reviewed change</button></footer>
</form></Dialog></template>
