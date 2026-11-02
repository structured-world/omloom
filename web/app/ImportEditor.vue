<script setup lang="ts">
import { ref, computed, onMounted, watch } from 'vue';
import Dialog from './Dialog.vue';
import { softwareFields } from './software-fields';
import { call, model, key, label, display, AssetKind } from './api';
import { equipmentFields } from './equipment-fields';
const props = defineProps<{ definitions: model.Definition[]; working: model.WorkingContext }>();
const emit = defineEmits<{ close: []; saved: [] }>();
const csv = ref(''); const source = ref(''); const kind = ref<number>(AssetKind.DEVICE); const preview = ref<model.Preview>(); const error = ref('');
const actions = ref<model.Actions>(); const parent = ref('');
const revision = ref<bigint>();
const available = computed(() => [...(actions.value?.creations ?? []), ...(actions.value?.allocations.filter(c => c.kind === AssetKind.PREFIX) ?? [])]);
function choiceContext(choice: model.Actions['creations'][number]) { return choice.parent ?? choice.prefixContext; }
const parents = computed(() => [...new Map(available.value.flatMap(c => {
  const context = choiceContext(c); return context?.target ? [[key(context.target), context] as const] : [];
})).values()]);
const choices = computed(() => available.value.filter(c => key(choiceContext(c)?.target) === parent.value));
const choice = computed(() => choices.value.find(c => c.kind === kind.value));
watch(choices, options => { if (!options.some(c => c.kind === kind.value) && options[0]) kind.value = options[0].kind; });
onMounted(async () => { try {
  const result = await call({ oneofKind: 'prepare', prepare: model.Prepare.create() }, model.WorkingContext.create({ ...props.working, expected: undefined }));
  if (result.oneofKind !== 'actions') throw Error('Import context is unavailable'); actions.value = result.actions; revision.value = result.actions.revision;
  if (parents.value.length === 1) parent.value = key(parents.value[0]!.target);
} catch (e) { error.value = String(e); } });
const mappings = ref([{ source: 'name', field: 'name' }]);
const networkInputs = [ { id: 'cidr', name: 'Prefix CIDR' }, { id: 'address_space', name: 'Address space reference' }, { id: 'broadcast', name: 'Explicit endpoint broadcast' }, { id: 'prefix', name: 'Selected prefix reference' }, { id: 'create_prefix', name: 'Ensure prefix membership' }, { id: 'prefix_name', name: 'Optional prefix name' } ];
const destinations = computed(() => [{ id: 'name', name: 'Name' }, { id: 'dns_name', name: 'DNS name' }, { id: 'description', name: 'Description' }, { id: 'serial', name: 'Serial number' }, { id: 'address', name: 'IP address' }, { id: 'prefix_length', name: 'Known prefix length' }, { id: 'meaning', name: 'Address meaning' }, { id: 'zone', name: 'Known local IPv6 zone' }, ...networkInputs, ...equipmentFields.map(id=>({id,name:id.replaceAll('_',' ')})), ...softwareFields(choice.value?.fields ?? []).filter(f=>!equipmentFields.includes(f.id)), ...props.definitions.filter(d => !d.retired && d.kinds.includes(kind.value))].filter(f => choice.value?.fields.includes(f.id)));
async function run(apply: boolean) {
  error.value = '';
  try {
    if (!actions.value || revision.value === undefined || !choice.value) throw Error('Choose an applicable kind and actual parent');
    if (!apply && mappings.value.some(m => !destinations.value.some(f => f.id === m.field))) throw Error('Review mappings that do not apply in this context');
    const prefix = choice.value.prefixContext?.target;
    const working = model.WorkingContext.create({ ...props.working, ...(prefix ? { roots: [prefix], focus: undefined } : {}), expected: apply ? preview.value?.revision : revision.value });
    const result = await call({ oneofKind: 'import', import: model.Import.create(apply ? { preview: preview.value?.id } : { csv: csv.value, source: source.value, kind: kind.value, mappings: mappings.value, parent: choice.value.parent?.target }) }, working);
    // Editing the draft clears its preview, but must retain the revision acknowledged by the store.
    if (apply) emit('saved'); else if (result.oneofKind === 'preview') { preview.value = result.preview; revision.value = result.preview.revision; }
  } catch (e) { error.value = String(e); }
}
</script>
<template><Dialog title="Import inventory" @close="emit('close')"><form @submit.prevent="run(false)">
<p class="hint">Map only the fields you intend to accept. Omitted fields are preserved; use &lt;clear&gt; to clear explicitly. The external_id column identifies rows within this personal source.</p>
<label>Source name<input v-model="source" required maxlength="100" @input="preview = undefined"></label>
<p v-if="parents.length === 1" class="context-chip">For {{ display(parents[0]!.fields.find(f => f.name === 'name')) || parents[0]!.target?.assetId.slice(-8) }}</p>
<label v-else-if="parents.length">Import context<select v-model="parent" required @change="preview = undefined"><option value="" disabled>Choose the import context</option><option v-for="p in parents" :key="key(p.target)" :value="key(p.target)">{{ display(p.fields.find(f => f.name === 'name')) || p.target?.assetId.slice(-8) }}</option></select></label>
<label>New resource kind<select v-model="kind" @change="preview = undefined"><option v-for="c in choices" :key="c.kind" :value="c.kind">{{ label(c.kind) }}</option></select></label>
<label>CSV<textarea v-model="csv" required rows="7" maxlength="262144" placeholder="external_id,name&#10;router-1,Edge router" @input="preview = undefined" /></label>
<div v-for="(mapping, i) in mappings" :key="i" class="row"><label>CSV column<input v-model="mapping.source" required @input="preview = undefined"></label><label>Inventory field<select v-model="mapping.field" @change="preview = undefined"><option v-for="d in destinations" :key="d.id" :value="d.id">{{ d.name }}</option></select></label><button v-if="i" type="button" @click="mappings.splice(i, 1); preview = undefined">Remove</button></div>
<button v-if="mappings.length < 32" type="button" @click="mappings.push({ source: '', field: 'description' }); preview = undefined">Add mapping</button>
<section v-if="preview" class="preview"><h3>Review {{ preview.rows.length }} rows</h3><p v-if="!preview.id" role="alert">Fix the errors before applying.</p><ol><li v-for="line in preview.rows" :key="line.row"><strong>Row {{ line.row }}</strong>: {{ line.diagnostic || (line.change?.target ? 'Update existing identity' : 'Create new identity') }}<span v-for="f in line.change?.changes" :key="f.name" class="chip">{{ destinations.find(d => d.id === f.name)?.name }}: {{ display(f) }}</span></li></ol></section>
<p v-if="error" role="alert">{{ error }}</p><footer><button type="button" @click="emit('close')">Cancel</button><button>Prepare preview</button><button v-if="preview?.id" type="button" class="primary" @click="run(true)">Accept reviewed rows</button></footer>
</form></Dialog></template>
