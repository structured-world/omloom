<script setup lang="ts">
import { ref, onMounted } from 'vue';
import Dialog from './Dialog.vue';
import { call, model, display, key, AssetKind } from './api';
const props = defineProps<{ detail: model.Detail }>();
const emit = defineEmits<{ close: []; saved: [] }>();
const items = ref<model.Item[]>([]); const selected = ref(''); const priority = ref<number>(); const error = ref('');
const isVrrp = props.detail.item?.fields.some(f => f.name === 'sharing' && display(f) === 'SHARED_USE_VRRP');
onMounted(async () => { try {
  const r = await call({ oneofKind: 'query', query: model.Query.create({ limit: 100, columns: ['name'], kinds: [AssetKind.INTERFACE] }) });
  if (r.oneofKind === 'page') items.value = r.page.items;
} catch (e) { error.value = String(e); } });
async function save() {
  try {
    const target = items.value.find(i => key(i.target) === selected.value)?.target;
    await call({ oneofKind: 'member', member: model.Member.create({ address: props.detail.item?.target, expected: props.detail.item?.revision, interface: target, priority: priority.value }) }); emit('saved');
  } catch (e) { error.value = String(e); }
}
</script>
<template><Dialog title="Assign address participant" @close="emit('close')"><form @submit.prevent="save"><p class="hint">An assignment does not claim a live interface, elected gateway or reachability.</p><label for="participant">Interface</label><select id="participant" v-model="selected" required><option value="">Select permitted interface</option><option v-for="i in items" :key="key(i.target)" :value="key(i.target)">{{ display(i.fields.find(f => f.name === 'name')) }} · {{ i.target?.assetId.slice(-8) }}</option></select><label v-if="isVrrp">Intended priority<input v-model.number="priority" type="number" min="1" max="255" required></label><p v-if="error" role="alert">{{ error }}</p><footer><button type="button" @click="emit('close')">Cancel</button><button class="primary">Assign participant</button></footer></form></Dialog></template>
