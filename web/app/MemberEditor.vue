<script setup lang="ts">
import { ref, onMounted } from 'vue';
import Dialog from './Dialog.vue';
import { call, model, display, key, AssetKind } from './api';
const props = defineProps<{ detail: model.Detail; working: model.WorkingContext }>();
const emit = defineEmits<{ close: []; saved: [] }>();
const items = ref<model.Item[]>([]); const selected = ref(''); const priority = ref<number>(); const error = ref('');
const search = ref(''); const next = ref<string>();
const revision = ref<bigint>();
const isVrrp = props.detail.item?.fields.some(f => f.name === 'sharing' && display(f) === 'SHARED_USE_VRRP');
async function load(cursor?: string) { try {
  const r = await call({ oneofKind: 'query', query: model.Query.create({ roots: props.working.roots, limit: 100, columns: ['name'], kinds: [AssetKind.INTERFACE], search: search.value, cursor }) });
  if (r.oneofKind !== 'page') throw Error('Unexpected interface response');
  items.value = r.page.items; next.value = r.page.next; selected.value = ''; revision.value = r.page.revision;
} catch (e) { error.value = String(e); } }
onMounted(() => load());
async function save() {
  try {
    const target = items.value.find(i => key(i.target) === selected.value)?.target;
    await call({ oneofKind: 'member', member: model.Member.create({ address: props.detail.item?.target, expected: props.detail.item?.revision, expectedGroup: props.detail.group?.revision, interface: target, priority: priority.value }) }, model.WorkingContext.create({ ...props.working, expected: revision.value })); emit('saved');
  } catch (e) { error.value = String(e); }
}
</script>
<template><Dialog title="Assign address participant" @close="emit('close')"><form @submit.prevent="save"><p class="hint">An assignment does not claim a live interface, elected gateway or reachability.</p><p v-if="detail.group" class="hint">This changes the shared group for all its addresses.</p><label>Find interface<input v-model="search" maxlength="256" @keydown.enter.prevent="load()"></label><button type="button" @click="load()">Find interfaces</button><label for="participant">Interface</label><select id="participant" v-model="selected" required><option value="">Select permitted interface</option><option v-for="i in items" :key="key(i.target)" :value="key(i.target)">{{ display(i.fields.find(f => f.name === 'name')) }} · {{ i.target?.assetId.slice(-8) }}</option></select><button v-if="next" type="button" @click="load(next)">Next interfaces</button><label v-if="isVrrp">Intended priority<input v-model.number="priority" type="number" min="1" max="255" required></label><p v-if="error" role="alert">{{ error }}</p><footer><button type="button" @click="emit('close')">Cancel</button><button class="primary">Assign participant</button></footer></form></Dialog></template>
