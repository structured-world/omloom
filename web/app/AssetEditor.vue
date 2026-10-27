<script setup lang="ts">
import { computed, onMounted, reactive, ref, watch } from 'vue';
import Dialog from './Dialog.vue';
import { call, typedField, input, label, key, model, AssetKind, type AssetRef } from './api';

const props = defineProps<{ item?: model.Item; definitions: model.Definition[]; initialKind?: AssetKind; initialHypervisor?: boolean; working: model.WorkingContext; view: model.View }>();
const emit = defineEmits<{ close: []; saved: [target?: AssetRef] }>();
const kind = ref<number | undefined>(props.item?.kind ?? props.initialKind);
const actions = ref<model.Actions>();
const selectedParent = ref('');
const values = reactive<Record<string, string>>({});
const modes = reactive<Record<string, string>>({});
const error = ref(''); const busy = ref(false); const preparing = ref(true); const more = ref(false);
type EditorField = { id: string; name: string; type: model.FieldType; choices: string[]; advanced: boolean };
const parents = computed(() => [...new Map(actions.value?.creations.flatMap(c => c.parent?.target ? [[key(c.parent.target), c.parent] as const] : [])).values()]);
const choices = computed(() => actions.value?.creations.filter(c => key(c.parent?.target) === selectedParent.value) ?? []);
const choice = computed(() => choices.value.find(c => c.kind === kind.value));
const known = computed(() => kind.value === AssetKind.ADDRESS_REFERENCE);
const fields = computed<EditorField[]>(() => {
  const allowed = props.item ? actions.value?.fields : choice.value?.fields;
  if (!allowed) return [];
  const builtins: EditorField[] = known.value ? [
    { id: 'address', name: 'IP address', type: model.FieldType.TEXT, choices: [], advanced: false },
    { id: 'prefix_length', name: 'Known prefix length', type: model.FieldType.INTEGER, choices: [], advanced: true },
    { id: 'meaning', name: 'Address meaning', type: model.FieldType.CHOICE, choices: Object.values(model.AddressMeaning).filter((v): v is number => typeof v === 'number').map(v => `ADDRESS_MEANING_${model.AddressMeaning[v]!}`), advanced: true },
    { id: 'zone', name: 'Known local IPv6 zone', type: model.FieldType.TEXT, choices: [], advanced: true },
    { id: 'description', name: 'Description', type: model.FieldType.TEXT, choices: [], advanced: true },
  ] : [
    { id: 'name', name: 'Name', type: model.FieldType.TEXT, choices: [], advanced: false },
    { id: 'description', name: 'Description', type: model.FieldType.TEXT, choices: [], advanced: false },
    { id: 'serial', name: 'Serial number', type: model.FieldType.TEXT, choices: [], advanced: false },
    { id: 'virtualization_role', name: 'Virtualization role', type: model.FieldType.CHOICE, choices: [`VIRTUALIZATION_ROLE_${model.VirtualizationRole[model.VirtualizationRole.HYPERVISOR]}`], advanced: false },
  ];
  const custom = props.definitions.filter(d => !d.retired && kind.value !== undefined && d.kinds.includes(kind.value))
    .map(d => ({ id: d.id, name: d.name, type: d.type, choices: d.choices, advanced: false }));
  return [...builtins, ...custom].filter(f => allowed.includes(f.id));
});
watch(fields, available => {
  for (const f of available) {
    if (!(f.id in values)) values[f.id] = input(props.item?.fields.find(v => v.name === f.id));
    if (!(f.id in modes)) modes[f.id] = props.item ? 'preserve' : ['name', 'address'].includes(f.id) ? 'set' : 'preserve';
    if (!props.item && props.initialHypervisor && f.id === 'virtualization_role' && values[f.id] === '') { values[f.id]=`VIRTUALIZATION_ROLE_${model.VirtualizationRole[model.VirtualizationRole.HYPERVISOR]}`; modes[f.id]='set'; }
  }
}, { immediate: true });
watch(choices, available => {
  if (!props.item && !available.some(c => c.kind === kind.value)) kind.value = available[0]?.kind;
});
const parentLabel = (parent: model.Item) => input(parent.fields.find(f => f.name === 'name')) || parent.target!.assetId.slice(-8);
onMounted(async () => {
  try {
    const working = model.WorkingContext.create({ ...props.working, focus: props.item?.target ?? props.working.focus, expected: undefined });
    const reply = await call({ oneofKind: 'prepare', prepare: model.Prepare.create({ view: props.view }) }, working);
    if (reply.oneofKind !== 'actions') throw Error('Creation context is unavailable');
    actions.value = reply.actions;
    if (!props.item && parents.value.length === 1) selectedParent.value = key(parents.value[0]!.target);
  } catch (e) { error.value = String(e); } finally { preparing.value = false; }
});
async function save() {
  busy.value = true; error.value = '';
  try {
    if (kind.value === undefined || !actions.value || (!props.item && !choice.value)) throw Error('Choose a resource and its actual parent');
    const changes = fields.value.filter(f => modes[f.id] !== 'preserve').map(f => typedField(f.id, f.type, values[f.id] ?? '', modes[f.id] === 'clear'));
    if (props.item && !changes.length) { emit('saved'); return; }
    const working = model.WorkingContext.create({ ...props.working, expected: actions.value.revision });
    const parent: AssetRef | undefined = props.item ? undefined : choice.value?.parent?.target;
    const reply = await call({ oneofKind: 'write', write: model.Write.create({ target: props.item?.target, expected: props.item?.revision, kind: kind.value, changes, parent }) }, working);
    if (reply.oneofKind !== 'mutation') throw Error('Unexpected resource response');
    emit('saved', props.item ? undefined : reply.mutation.targets[0]);
  } catch (e) { error.value = String(e); } finally { busy.value = false; }
}
</script>
<template><Dialog :title="item ? 'Edit resource' : known ? 'Record known address' : 'Create resource'" @close="emit('close')"><form @submit.prevent="save">
<p v-if="preparing" role="status">Checking available actions…</p>
<template v-else>
<p v-if="known" class="hint">Record what you know. Network, VRF and mask may remain unknown. This does not reserve an address.</p>
<p v-else class="hint">This action changes accepted inventory in the selected context.</p>
<p v-if="item" class="context-chip">Editing {{ input(item.fields.find(f => f.name === 'name')) || item.target?.assetId.slice(-8) }}</p>
<p v-else-if="parents.length === 1" class="context-chip">Inside {{ parentLabel(parents[0]!) }} · {{ label(parents[0]!.kind) }}</p>
<label v-else-if="parents.length" for="creation-parent">Create inside</label>
<select v-if="!item && parents.length > 1" id="creation-parent" v-model="selectedParent" required><option value="" disabled>Choose the actual parent</option><option v-for="p in parents" :key="key(p.target)" :value="key(p.target)">{{ parentLabel(p) }} · {{ label(p.kind) }}</option></select>
<label for="resource-kind">Resource kind</label>
<select id="resource-kind" v-model="kind" :disabled="!!item || busy" required>
<option v-if="item" :value="item.kind">{{ label(item.kind) }}</option>
<option v-else-if="!choices.length" disabled value="">No available action in this context</option>
<option v-for="c in item ? [] : choices" :key="c.kind" :value="c.kind">{{ label(c.kind) }}</option>
</select>
<button v-if="known" type="button" :aria-expanded="more" @click="more = !more">{{ more ? 'Hide additional facts' : 'More known details' }}</button>
<template v-for="f in fields" :key="f.id"><div v-if="!f.advanced || more" class="field-edit">
<label :for="'value-'+f.id">{{ f.name }}</label>
<select v-if="item || !['name','address'].includes(f.id)" v-model="modes[f.id]" :aria-label="f.name+' change'"><option value="preserve">{{ item ? 'Preserve' : 'Leave unknown' }}</option><option value="set">Set</option><option v-if="!['name','address','meaning'].includes(f.id)" value="clear">Clear explicitly</option></select>
<select v-if="f.type === model.FieldType.BOOLEAN" :id="'value-'+f.id" v-model="values[f.id]" :disabled="modes[f.id] !== 'set'" :required="modes[f.id] === 'set'"><option disabled value="">Choose true or false</option><option value="true">True</option><option value="false">False</option></select>
<select v-else-if="f.choices.length" :id="'value-'+f.id" v-model="values[f.id]" :disabled="modes[f.id] !== 'set'" :required="modes[f.id] === 'set'"><option disabled value="">Choose a value</option><option v-for="value in f.choices" :key="value" :value="value">{{ f.id === 'meaning' ? value.replace('ADDRESS_MEANING_', '').toLowerCase() : value }}</option></select>
<input v-else :id="'value-'+f.id" v-model="values[f.id]" :type="f.type === model.FieldType.DATE ? 'date' : f.type === model.FieldType.INTEGER ? 'number' : 'text'" :disabled="modes[f.id] !== 'set'" :required="modes[f.id] === 'set' && ['name','address'].includes(f.id)" :maxlength="f.id === 'name' ? 160 : f.id === 'address' ? 64 : 2048" @input="modes[f.id] = 'set'">
</div></template>
</template>
<p v-if="error" role="alert">{{ error }}</p><footer><button type="button" @click="emit('close')">Cancel</button><button class="primary" :disabled="busy || preparing || (!item && !choice)">{{ busy ? 'Saving…' : known ? 'Save known address' : 'Save resource' }}</button></footer>
</form></Dialog></template>
