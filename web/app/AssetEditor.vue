<script setup lang="ts">
import { reactive, ref } from 'vue';
import Dialog from './Dialog.vue';
import { call, typedField, input, label, kinds, model, AssetKind } from './api';
const props = defineProps<{ item?: model.Item; definitions: model.Definition[] }>();
const emit = defineEmits<{ close: []; saved: [] }>();
const kind = ref(props.item?.kind ?? AssetKind.DEVICE);
const values = reactive<Record<string, string>>({});
const modes = reactive<Record<string, string>>({});
const fields = [{ id: 'name', name: 'Name', type: model.FieldType.TEXT }, { id: 'description', name: 'Description', type: model.FieldType.TEXT }, { id: 'serial', name: 'Serial number', type: model.FieldType.TEXT }, ...props.definitions.filter(d => !d.retired)];
for (const f of fields) { values[f.id] = input(props.item?.fields.find(v => v.name === f.id)); modes[f.id] = props.item ? 'preserve' : f.id === 'name' ? 'set' : 'preserve'; }
const error = ref(''); const busy = ref(false);
async function save() {
  busy.value = true; error.value = '';
  try {
    const changes = fields.filter(f => modes[f.id] !== 'preserve').map(f => typedField(f.id, f.type, values[f.id] ?? '', modes[f.id] === 'clear'));
    await call({ oneofKind: 'write', write: model.Write.create({ target: props.item?.target, expected: props.item?.revision, kind: kind.value, changes }) }); emit('saved');
  } catch (e) { error.value = String(e); } finally { busy.value = false; }
}
</script>
<template><Dialog :title="item ? 'Edit resource' : 'Create resource'" @close="emit('close')"><form @submit.prevent="save">
<p class="hint">Changes affect accepted inventory. Observations and import proposals remain separate.</p>
<label for="resource-kind">Resource kind</label><select id="resource-kind" v-model="kind" :disabled="!!item"><option v-for="k in kinds" :key="k" :value="k" :disabled="!item && [AssetKind.IP_ADDRESS, AssetKind.PREFIX, AssetKind.VLAN, AssetKind.ADDRESS_GROUP].includes(k)">{{ label(k) }}{{ !item && [AssetKind.IP_ADDRESS, AssetKind.PREFIX, AssetKind.VLAN, AssetKind.ADDRESS_GROUP].includes(k) ? ' (use Network allocation)' : '' }}</option></select>
<template v-for="f in fields" :key="f.id"><div v-if="!('kinds' in f) || f.kinds.includes(kind)" class="field-edit">
<label :for="`value-${f.id}`">{{ f.name }}</label><select v-model="modes[f.id]" :aria-label="`${f.name} change`"><option value="preserve">Preserve</option><option value="set">Set</option><option v-if="f.id !== 'name'" value="clear">Clear explicitly</option></select>
<select v-if="f.type === model.FieldType.BOOLEAN" :id="`value-${f.id}`" v-model="values[f.id]" :disabled="modes[f.id] !== 'set'" :required="modes[f.id] === 'set'"><option disabled value="">Choose true or false</option><option value="true">True</option><option value="false">False</option></select>
<select v-else-if="'choices' in f && f.choices.length" :id="`value-${f.id}`" v-model="values[f.id]" :disabled="modes[f.id] !== 'set'" :required="modes[f.id] === 'set'"><option disabled value="">Choose a value</option><option v-for="choice in f.choices" :key="choice">{{ choice }}</option></select>
<input v-else :id="`value-${f.id}`" v-model="values[f.id]" :type="f.type === model.FieldType.DATE ? 'date' : f.type === model.FieldType.INTEGER ? 'number' : 'text'" :disabled="modes[f.id] !== 'set'" :maxlength="f.id === 'name' ? 160 : 2048" @input="modes[f.id] = 'set'">
</div></template>
<p v-if="error" role="alert">{{ error }}</p><footer><button type="button" @click="emit('close')">Cancel</button><button class="primary" :disabled="busy">{{ busy ? 'Saving…' : 'Save resource' }}</button></footer>
</form></Dialog></template>
