<script setup lang="ts">
import { onMounted, ref } from 'vue';
import Dialog from './Dialog.vue';
import { call, model, key, label, display, RelationKind, type AssetRef } from './api';
const props = defineProps<{ target: AssetRef }>();
const emit = defineEmits<{ close: []; saved: [] }>();
const search = ref(''); const next = ref<string>(); const items = ref<model.Item[]>([]);
const destination = ref(''); const kind = ref(RelationKind.INSTALLED_IN); const error = ref('');
async function load(cursor?: string) {
  try {
    error.value = '';
    const r = await call({ oneofKind: 'query', query: model.Query.create({ limit: 100, columns: ['name'], search: search.value, cursor }) });
    if (r.oneofKind !== 'page') throw Error('Unexpected inventory response');
    items.value = r.page.items.filter(i => key(i.target) !== key(props.target)); next.value = r.page.next; destination.value = '';
  } catch (e) { error.value = String(e); }
}
async function save() {
  try {
    const to = items.value.find(i => key(i.target) === destination.value)?.target;
    if (!to) throw Error('Select a permitted destination');
    await call({ oneofKind: 'relation', relation: { relation: { relationId: '', from: props.target, to, kind: kind.value, revision: 0n, authorityId: '' }, remove: false } }); emit('saved');
  } catch (e) { error.value = String(e); }
}
onMounted(() => load());
</script>
<template><Dialog title="Add typed relationship" @close="emit('close')"><form @submit.prevent="save">
  <label>Relationship<select v-model="kind"><option v-for="r in Object.values(RelationKind).filter((r): r is number => typeof r === 'number' && r > 0)" :key="r" :value="r">{{ RelationKind[r]?.replace('RELATION_KIND_', '').toLowerCase().replaceAll('_',' ') }}</option></select></label>
  <label>Find destination<input v-model="search" maxlength="256" @keydown.enter.prevent="load()"></label><button type="button" @click="load()">Find resources</button>
  <label>Destination<select v-model="destination" required><option value="">Select permitted resource</option><option v-for="i in items" :key="key(i.target)" :value="key(i.target)">{{ display(i.fields.find(f => f.name === 'name')) }} · {{ label(i.kind) }}</option></select></label>
  <button v-if="next" type="button" @click="load(next)">Next destinations</button>
  <p class="hint">The server checks both endpoints, resource kinds and current write permissions. Search or use the next page for more destinations.</p><p v-if="error" role="alert">{{ error }}</p>
  <footer><button type="button" @click="emit('close')">Cancel</button><button class="primary">Add relationship</button></footer>
</form></Dialog></template>
