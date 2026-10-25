<script setup lang="ts">
import { ref } from 'vue';
import Dialog from './Dialog.vue';
import { call, model } from './api';
const props = defineProps<{ query: model.Query; identity: model.Identity; existing?: model.SavedView }>();
const emit = defineEmits<{ close: []; saved: [] }>();
const name = ref(props.existing?.name ?? ''); const audiences = ref([...(props.existing?.audiences ?? [])]); const editors = ref([...(props.existing?.editors ?? [])]); const reviewed = ref(false); const error = ref('');
async function save(publish: boolean) {
  try {
    await call({ oneofKind: 'saveView', saveView: model.SaveView.create({ view: model.SavedView.create({ ...props.existing, name: name.value, query: props.query, audiences: audiences.value, editors: editors.value }), publish }) });
    if (audiences.value.length && !publish) reviewed.value = true; else emit('saved');
  } catch (e) { error.value = String(e); }
}
</script>
<template><Dialog title="Save workspace view" @close="emit('close')"><form @submit.prevent="save(false)"><p class="hint">A personal view saves this selection and columns, not results. Publishing its definition never grants access to inventory.</p><label>Name<input v-model="name" required maxlength="100" @input="reviewed = false"></label><fieldset><legend>Definition audience</legend><p v-if="!audiences.length">Personal only</p><label v-for="team in identity.teams" :key="team" class="check"><input v-model="audiences" type="checkbox" :value="team" @change="reviewed = false">{{ team }}</label></fieldset><fieldset v-if="audiences.length"><legend>Team editors</legend><label v-for="team in audiences" :key="team" class="check"><input v-model="editors" type="checkbox" :value="team" @change="reviewed = false">{{ team }} may edit this definition</label></fieldset><p v-if="reviewed" class="notice">Publish the name, selected roots, predicates and columns to {{ audiences.join(', ') }}. Data remains restricted by each reader's rights.</p><p v-if="error" role="alert">{{ error }}</p><footer><button type="button" @click="emit('close')">Cancel</button><button v-if="reviewed" type="button" class="primary" @click="save(true)">Publish reviewed definition</button><button v-else class="primary">{{ audiences.length ? 'Preview publication' : 'Save personal view' }}</button></footer></form></Dialog></template>
