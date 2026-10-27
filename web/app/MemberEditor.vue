<script setup lang="ts">
import { ref, onMounted, computed, watch } from 'vue';
import Dialog from './Dialog.vue';
import { call, model, display, key, AssetKind, type AssetRef } from './api';
const props = defineProps<{ detail: model.Detail; working: model.WorkingContext }>();
const emit = defineEmits<{ close: []; saved: [] }>();
const items = ref<model.Item[]>([]); const selected = ref(''); const priority = ref<number>(); const error = ref('');
const search = ref(''); const next = ref<string>();
const bounds = ref([...props.working.roots]);
const reviewed = ref<model.WorkingContext>(); const busy = ref(false);
const chosen = computed(() => items.value.find(i => key(i.target) === selected.value));
let generation = 0;
watch([selected, priority], () => { generation++; reviewed.value = undefined; }, { flush: 'sync' });
const isVrrp = props.detail.item?.fields.some(f => f.name === 'sharing' && display(f) === 'SHARED_USE_VRRP');
async function load(cursor?: string) { if (busy.value) return; busy.value = true; error.value = ''; reviewed.value = undefined; try {
  const r = await call({ oneofKind: 'query', query: model.Query.create({ roots: bounds.value, limit: 100, columns: ['name'], kinds: [AssetKind.INTERFACE], search: search.value, cursor }) });
  if (r.oneofKind !== 'page') throw Error('Unexpected interface response');
  items.value = r.page.items; next.value = r.page.next; selected.value = '';
} catch (e) { error.value = String(e); } finally { busy.value = false; } }
onMounted(() => load());
async function review() {
  if (busy.value) return;
  busy.value = true; error.value = ''; reviewed.value = undefined;
  const turn = ++generation;
  try {
    if (!props.detail.item?.target || !chosen.value?.target) throw Error('Choose an interface before reviewing the assignment');
    const roots = [props.detail.item.target, props.detail.group?.target, chosen.value.target].filter((t): t is AssetRef => !!t);
    const working = model.WorkingContext.create({ roots: [...new Map(roots.map(t => [key(t), t])).values()] });
    const r = await call({ oneofKind: 'prepare', prepare: model.Prepare.create({ view: model.View.NETWORK }) }, working);
    if (r.oneofKind !== 'actions') throw Error('Participant context review is unavailable');
    if (turn === generation) reviewed.value = model.WorkingContext.create({ ...working, expected: r.actions.revision });
  } catch (e) { error.value = String(e); } finally { busy.value = false; }
}
async function save() {
  if (busy.value) return;
  busy.value = true; error.value = '';
  try {
    if (!reviewed.value || !chosen.value?.target) throw Error('Review the participant context before assigning');
    await call({ oneofKind: 'member', member: model.Member.create({ address: props.detail.item?.target, expected: props.detail.item?.revision, expectedGroup: props.detail.group?.revision, interface: chosen.value.target, priority: priority.value }) }, reviewed.value); emit('saved');
  } catch (e) { error.value = String(e); } finally { busy.value = false; }
}
</script>
<template><Dialog title="Assign address participant" @close="emit('close')"><form @submit.prevent="reviewed ? save() : review()">
<p class="hint">An assignment does not claim a live interface, elected gateway or reachability.</p><p v-if="detail.group" class="hint">This changes the shared group for all its addresses.</p>
<p>{{ bounds.length ? 'Finding interfaces within the enclosing selection.' : 'Finding interfaces across permitted inventory.' }}</p>
<button v-if="bounds.length" type="button" :disabled="busy" @click="bounds = []; load()">Choose across permitted inventory</button>
<label>Find interface<input v-model="search" maxlength="256" @keydown.enter.prevent="load()"></label><button type="button" :disabled="busy" @click="load()">Find interfaces</button>
<label for="participant">Interface</label><select id="participant" v-model="selected" required><option value="">Select permitted interface</option><option v-for="i in items" :key="key(i.target)" :value="key(i.target)">{{ display(i.fields.find(f => f.name === 'name')) }} · {{ i.target?.assetId.slice(-8) }}</option></select><button v-if="next" type="button" :disabled="busy" @click="load(next)">Next interfaces</button>
<label v-if="isVrrp">Intended priority<input v-model.number="priority" type="number" min="1" max="255" required></label>
<section v-if="reviewed" aria-label="Reviewed participant context"><p>This assignment uses only the selected allocation, its shared group and this interface. Each keeps its own permissions; browsing scope stays unchanged.</p><ul><li v-for="i in [detail.item, detail.group, chosen].filter(Boolean)" :key="key(i?.target)">{{ display(i?.fields.find(f => f.name === 'name')) }} · {{ i?.target?.assetId }}</li></ul></section>
<p v-if="error" role="alert">{{ error }}</p><footer><button type="button" @click="emit('close')">Cancel</button><button class="primary" :disabled="busy || !chosen">{{ reviewed ? 'Assign participant' : 'Review participant assignment' }}</button></footer>
</form></Dialog></template>
