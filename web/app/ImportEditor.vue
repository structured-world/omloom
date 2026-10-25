<script setup lang="ts">
import { ref, computed } from 'vue';
import Dialog from './Dialog.vue';
import { call, model, kinds, label, display } from './api';
const props = defineProps<{ definitions: model.Definition[] }>();
const emit = defineEmits<{ close: []; saved: [] }>();
const csv = ref(''); const source = ref(''); const kind = ref(3); const preview = ref<model.Preview>(); const error = ref('');
const mappings = ref([{ source: 'name', field: 'name' }]);
const destinations = computed(() => [{ id: 'name', name: 'Name' }, { id: 'description', name: 'Description' }, { id: 'serial', name: 'Serial number' }, ...props.definitions.filter(d => !d.retired)]);
async function run(apply: boolean) {
  error.value = '';
  try {
    const result = await call({ oneofKind: 'import', import: model.Import.create(apply ? { preview: preview.value?.id } : { csv: csv.value, source: source.value, kind: kind.value, mappings: mappings.value }) });
    if (apply) emit('saved'); else if (result.oneofKind === 'preview') preview.value = result.preview;
  } catch (e) { error.value = String(e); }
}
</script>
<template><Dialog title="Import inventory" @close="emit('close')"><form @submit.prevent="run(false)">
<p class="hint">Map only the fields you intend to accept. Omitted fields are preserved; use &lt;clear&gt; to clear explicitly. The external_id column identifies rows within this personal source.</p>
<label>Source name<input v-model="source" required maxlength="100" @input="preview = undefined"></label>
<label>New resource kind<select v-model="kind" @change="preview = undefined"><option v-for="k in kinds" :key="k" :value="k">{{ label(k) }}</option></select></label>
<label>CSV<textarea v-model="csv" required rows="7" maxlength="262144" placeholder="external_id,name&#10;router-1,Edge router" @input="preview = undefined" /></label>
<div v-for="(mapping, i) in mappings" :key="i" class="row"><label>CSV column<input v-model="mapping.source" required @input="preview = undefined"></label><label>Inventory field<select v-model="mapping.field" @change="preview = undefined"><option v-for="d in destinations" :key="d.id" :value="d.id">{{ d.name }}</option></select></label><button v-if="i" type="button" @click="mappings.splice(i, 1); preview = undefined">Remove</button></div>
<button v-if="mappings.length < 32" type="button" @click="mappings.push({ source: '', field: 'description' }); preview = undefined">Add mapping</button>
<section v-if="preview" class="preview"><h3>Review {{ preview.rows.length }} rows</h3><p v-if="!preview.id" role="alert">Fix the errors before applying.</p><ol><li v-for="line in preview.rows" :key="line.row"><strong>Row {{ line.row }}</strong>: {{ line.diagnostic || (line.change?.target ? 'Update existing identity' : 'Create new identity') }}<span v-for="f in line.change?.changes" :key="f.name" class="chip">{{ destinations.find(d => d.id === f.name)?.name }}: {{ display(f) }}</span></li></ol></section>
<p v-if="error" role="alert">{{ error }}</p><footer><button type="button" @click="emit('close')">Cancel</button><button>Prepare preview</button><button v-if="preview?.id" type="button" class="primary" @click="run(true)">Accept reviewed rows</button></footer>
</form></Dialog></template>
