<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, useId, watch } from 'vue';
import { call, model, AssetKind as K, input, key, type AssetRef } from './api';
const props = defineProps<{ kind: 'server' | 'hypervisor'; roots: AssetRef[] }>();
const emit = defineEmits<{ select: [item?: model.Item]; ready: [value: boolean] }>();
const search = ref(''); const selected = ref<model.Item>(); const opened = ref(false);
const page = ref(model.Page.create()); const loading = ref(false); const error = ref('');
const active = ref(0); const control = ref<HTMLInputElement>(); const id = useId();
const title = computed(() => props.kind === 'server' ? 'Server' : 'Hypervisor');
let generation = 0; let pending: ReturnType<typeof setTimeout> | undefined;
const name = (item: model.Item) => input(item.fields.find(f => f.name === 'name')) || item.target!.assetId.slice(-8);
function cancel() { generation++; if (pending !== undefined) clearTimeout(pending); pending = undefined; loading.value = false; }
function reset() {
  cancel(); selected.value = undefined; search.value = ''; opened.value = false;
  page.value = model.Page.create(); error.value = ''; emit('select'); emit('ready', true);
}
watch(() => props.kind, reset);
watch(() => props.roots.map(key).join('|'), reset);
onMounted(() => emit('ready', true));
onBeforeUnmount(cancel);
async function load(cursor?: string) {
  const turn = ++generation; loading.value = true; error.value = '';
  try {
    const reply = await call({ oneofKind: 'query', query: model.Query.create({
      roots: props.roots, view: props.kind === 'server' ? model.View.HARDWARE : model.View.VIRTUALIZATION,
      kinds: [props.kind === 'server' ? K.DEVICE : K.SOFTWARE_INSTALLATION],
      columns: ['name'], search: search.value, cursor, limit: 25, includeContext: true,
    }) });
    if (reply.oneofKind !== 'page') throw Error('Placement choices are unavailable');
    if (turn === generation) { page.value = reply.page; active.value = search.value && reply.page.items.length ? 1 : 0; }
  } catch (e) { if (turn === generation) { error.value = String(e); page.value = model.Page.create(); } }
  finally { if (turn === generation) loading.value = false; }
}
function edit() {
  if (!search.value && selected.value) choose();
  cancel(); opened.value = true; page.value = model.Page.create(); error.value = '';
  emit('ready', search.value === (selected.value ? name(selected.value) : ''));
  pending = setTimeout(() => { pending = undefined; void load(); }, 250);
}
function choose(item?: model.Item) {
  cancel(); selected.value = item; search.value = item ? name(item) : ''; opened.value = false;
  emit('select', item); emit('ready', true);
}
function close() { cancel(); opened.value = false; search.value = selected.value ? name(selected.value) : ''; emit('ready', true); }
function focus() { opened.value = true; void load(); }
function keyboard(event: KeyboardEvent) {
  if (event.key === 'Escape') { event.preventDefault(); event.stopPropagation(); close(); }
  else if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
    event.preventDefault();
    if (!opened.value) focus();
    else active.value = Math.max(0, Math.min(page.value.items.length, active.value + (event.key === 'ArrowDown' ? 1 : -1)));
  } else if (event.key === 'Enter') {
    event.preventDefault();
    if (!opened.value) focus();
    else if (!loading.value && pending === undefined) choose(active.value === 0 ? undefined : page.value.items[active.value - 1]);
  }
}
function owner(item: model.Item) {
  return page.value.contexts.find(c => key(c.target) === key(item.target))?.owners.map(o => o.name).filter(Boolean).join(' / ');
}
</script>
<template><div class="placement-search">
<label :for="id">{{ title }}</label>
<div class="placement-control"><input :id="id" ref="control" v-model="search" role="combobox" :aria-label="'Find '+kind" placeholder="Unknown" autocomplete="off" maxlength="256" aria-autocomplete="list" :aria-controls="id+'-results'" :aria-expanded="opened" :aria-activedescendant="opened && !loading ? id+'-option-'+active : undefined" @focus="focus" @input="edit" @blur="close" @keydown="keyboard"><button v-if="search || selected" type="button" :aria-label="'Clear '+kind" @mousedown.prevent @click="choose(); control?.focus()">×</button></div>
<div v-if="opened" class="placement-popup">
<p v-if="loading" role="status">Searching…</p>
<ul :id="id+'-results'" role="listbox" :aria-label="title+' choices'">
<li><button :id="id+'-option-0'" role="option" type="button" :aria-selected="!selected" :class="{ active: active === 0 }" @mousedown.prevent @click="choose()">Unknown</button></li>
<li v-for="(item, index) in loading ? [] : page.items" :key="key(item.target)"><button :id="id+'-option-'+(index+1)" role="option" type="button" :aria-selected="key(selected?.target) === key(item.target)" :aria-label="'Use '+kind+': '+name(item)" :class="{ active: active === index+1 }" @mousedown.prevent @click="choose(item)"><strong>{{ name(item) }}</strong><small v-if="owner(item)">{{ owner(item) }}</small></button></li>
</ul>
<p v-if="!loading && !error && !page.items.length" class="hint">No matching {{ kind === 'server' ? 'servers' : 'hypervisors' }} in this scope.</p>
<button v-if="!loading && page.next" type="button" @mousedown.prevent @click="load(page.next)">Next placement choices</button>
<p v-if="error" role="alert">{{ error }}</p>
</div></div></template>
<style scoped>
.placement-search { position:relative;margin:16px 0;min-width:0; }
.placement-search > label { margin-bottom:6px; }
.placement-control { display:flex;border:1px solid #cbd6d9;border-radius:4px;background:#fff; }
.placement-control:focus-within { outline:3px solid #169c91;outline-offset:2px; }
.placement-control input { width:100%;min-width:0;border:0;outline:0; }
.placement-control button { border:0;min-width:44px; }
.placement-popup { position:absolute;top:100%;left:0;right:0;z-index:3;max-height:280px;overflow:auto;padding:6px;background:#fff;border:1px solid #cbd6d9;border-radius:6px;box-shadow:0 8px 24px #102e3a22; }
.placement-popup ul { list-style:none;margin:0;padding:0; }
.placement-popup [role=option] { display:flex;flex-direction:column;align-items:flex-start;text-align:left;width:100%;min-height:44px;border:0;overflow-wrap:anywhere; }
.placement-popup .active { background:#e7f1f1; }
.placement-popup p { padding:6px;margin:0; }
</style>
