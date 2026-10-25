<script setup lang="ts">
import { ref, reactive } from 'vue';
import Dialog from './Dialog.vue';
import { call, model, kinds, label } from './api';
const props = defineProps<{ definitions: model.Definition[]; revision: bigint }>();
const emit = defineEmits<{ close: []; saved: [] }>();
const selected = ref('');
const draft = reactive(model.Definition.create());
const choices = ref(''); const preview = ref<bigint>(); const convert = ref(false); const error = ref('');
function load() { const selectedDefinition = props.definitions.find(d => d.id === selected.value); Object.assign(draft, selectedDefinition ? model.Definition.create(selectedDefinition) : model.Definition.create()); choices.value = draft.choices.join('\n'); preview.value = undefined; }
async function save(apply = false) {
  error.value = '';
  try {
    draft.choices = draft.type === model.FieldType.CHOICE ? choices.value.split('\n').filter(Boolean) : [];
    const old = props.definitions.find(d => d.id === draft.id);
    const migration = old && (old.type !== draft.type || JSON.stringify(old.kinds) !== JSON.stringify(draft.kinds) || JSON.stringify(old.choices) !== JSON.stringify(draft.choices));
    const result = migration ? await call({ oneofKind: 'migration', migration: model.Migration.create({ definition: draft, expected: props.revision, apply, convertTypes: convert.value }) })
      : await call({ oneofKind: 'definition', definition: draft });
    if (migration && !apply && result.oneofKind === 'mutation') { preview.value = result.mutation.affected; return; }
    emit('saved');
  } catch (e) { error.value = String(e); }
}
</script>
<template><Dialog title="Company fields" @close="emit('close')"><form @submit.prevent="save(false)" @input="preview = undefined" @change="preview = undefined">
<label>Definition<select v-model="selected" @change="load"><option value="">New field</option><option v-for="d in definitions" :key="d.id" :value="d.id">{{ d.name }}{{ d.retired ? ' (retired)' : '' }}</option></select></label>
<label>Name<input v-model="draft.name" required maxlength="100"></label><label>Help<textarea v-model="draft.description" maxlength="1000" /></label>
<label for="definition-type">Type</label><select id="definition-type" v-model="draft.type" @change="preview = undefined"><option v-for="t in Object.values(model.FieldType).filter((t): t is number => typeof t === 'number')" :key="t" :value="t">{{ model.FieldType[t]?.toLowerCase() }}</option></select>
<label v-if="draft.type === model.FieldType.CHOICE">Choices, one per line<textarea v-model="choices" /></label>
<fieldset><legend>Applies to</legend><label v-for="k in kinds" :key="k" class="check"><input v-model="draft.kinds" type="checkbox" :value="k">{{ label(k) }}</label></fieldset>
<label class="check"><input v-model="draft.searchable" type="checkbox">Include values in descriptive search</label>
<label class="check"><input v-model="draft.retired" type="checkbox">Retire, preserving current interpretation</label>
<label v-if="draft.id" class="check"><input v-model="convert" type="checkbox">Review deterministic conversion of existing values</label>
<p v-if="preview !== undefined" class="notice">{{ preview }} current values validated against the next definition. Apply commits the definition and values atomically.</p>
<p v-if="error" role="alert">{{ error }}</p><footer><button type="button" @click="emit('close')">Cancel</button><button v-if="preview !== undefined" type="button" class="primary" @click="save(true)">Apply reviewed migration</button><button v-else class="primary">Review / save definition</button></footer>
</form></Dialog></template>
