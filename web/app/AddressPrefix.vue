<script setup lang="ts">
import { computed, ref, useId, watch } from 'vue';
import { call, model, AssetKind as K, input, key } from './api';
const props = defineProps<{ address: string; prefixLength?: number; editing?: boolean; initialPrefix?: model.Item; working: model.WorkingContext }>();
const id = useId();
const choice = defineModel<Partial<model.Address>>({ required: true });
const mode = ref(props.editing ? 'preserve' : props.initialPrefix ? 'existing' : 'new');
const name = ref(''); const search = ref(''); const selected = ref(props.initialPrefix ? key(props.initialPrefix.target) : '');
const candidates = ref<model.Item[]>(props.initialPrefix ? [props.initialPrefix] : []); const cursor = ref<string>(); const error = ref('');
let searchTurn = 0;
watch(search, () => { searchTurn++; cursor.value = undefined; error.value = ''; }, { flush: 'sync' });
const cidr = computed(() => props.prefixLength !== undefined || /\/\d+/.test(props.address));
function update() {
  const result: Partial<model.Address> = mode.value === 'new' && cidr.value ? { createPrefix: true, prefixName: name.value || undefined }
    : mode.value === 'existing' ? { prefix: candidates.value.find(i => key(i.target) === selected.value)?.target }
    : mode.value === 'none' && props.editing ? { clearPrefix: true } : {};
  choice.value = result;
  return result;
}
async function load(more = false) {
  const next = more ? cursor.value : undefined;
  if (more && !next) return;
  const turn = ++searchTurn;
  try {
    error.value = '';
    const result = await call({ oneofKind: 'query', query: model.Query.create({
      kinds: [K.PREFIX], columns: ['name','cidr','address_space'], search: search.value,
      limit: 3, cursor: next,
    }) });
    if (turn !== searchTurn) return;
    if (result.oneofKind !== 'page') throw Error('Prefixes are unavailable');
    const kept = candidates.value.find(item => key(item.target) === selected.value);
    if (!more && kept && !result.page.items.some(item => key(item.target) === selected.value)) result.page.items.unshift(kept);
    if (more && candidates.value.length + result.page.items.length > 128) throw Error('Narrow the search to find another prefix. The current selection is preserved.');
    candidates.value = more ? [...candidates.value, ...result.page.items] : result.page.items;
    cursor.value = result.page.next;
  } catch (e) { if (turn === searchTurn) error.value = String(e); }
}
defineExpose({ request: () => { const result = update(); if (mode.value === 'existing' && !result.prefix) throw Error('Select an existing prefix'); return result; } });
</script>
<template><section aria-label="Prefix knowledge" class="prefix-choice">
<label :for="id+'-mode'">Prefix relationship</label><select :id="id+'-mode'" v-model="mode" @change="update(); mode === 'existing' && load()">
<option v-if="editing" value="preserve">Keep the current relationship</option>
<option v-if="!initialPrefix" value="new">New prefix, namespace unknown</option>
<option value="existing">Choose an existing prefix</option>
<option v-if="!initialPrefix || editing" value="none">{{ editing ? 'Remove the relationship only' : 'Record the address only' }}</option>
</select>
<template v-if="mode === 'new'"><p v-if="!cidr">Without a mask, only the address will be recorded.</p>
<template v-else><label>Prefix name (optional)<input v-model="name" maxlength="160" @input="update"></label><p>A new prefix and this address will be saved together. No namespace, interface or allocation is inferred.</p></template></template>
<template v-if="mode === 'existing'">
<p>Choose explicitly across permitted prefixes. Equal address text does not establish network identity.</p>
<label>Find a prefix<input v-model="search" maxlength="256"></label><button type="button" @click="load()">Find prefixes</button>
<label :for="id+'-existing'">Existing prefix</label><select :id="id+'-existing'" v-model="selected" @change="update"><option value="">Select a prefix</option><option v-for="item in candidates" :key="key(item.target)" :value="key(item.target)">{{ input(item.fields.find(f => f.name === 'cidr')) }} · {{ input(item.fields.find(f => f.name === 'address_space')) || 'Namespace not shown' }} · {{ input(item.fields.find(f => f.name === 'name')) || item.target?.assetId.slice(-8) }}</option></select>
<button v-if="cursor" type="button" @click="load(true)">More prefixes</button></template>
<p v-if="error" role="alert">{{ error }}</p>
</section></template>
<style scoped>.prefix-choice { margin-block:1rem;padding:1rem;border:1px solid #bed0d5;border-radius:.5rem; } .prefix-choice select,.prefix-choice input { width:100%;min-width:0; } p { overflow-wrap:anywhere; }</style>
