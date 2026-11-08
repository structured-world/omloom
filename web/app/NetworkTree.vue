<script setup lang="ts">
import { onScopeDispose, ref, watch } from 'vue';
import { AssetKind as K, call, identityName, key, label, model, type AssetRef } from './api';

const props = defineProps<{ query: model.Query; refresh: number; selected?: AssetRef; ancestors: string[] }>();
const emit = defineEmits<{ select: [item: model.Item]; inspect: [target: AssetRef] }>();
const containers = new Set([K.SITE, K.LOCATION, K.RACK, K.DEVICE, K.VIRTUAL_MACHINE, K.SOFTWARE_INSTALLATION,
  K.SERVICE, K.WORKLOAD, K.CONTAINER, K.INTERFACE, K.PORT, K.PHYSICAL_COMPONENT, K.PHYSICAL_POSITION,
  K.ADDRESS_SPACE, K.PREFIX, K.L2_DOMAIN, K.SOFTWARE_PRODUCT]);
const items = ref<model.Item[]>([]);
const next = ref<string>();
const cursor = ref<string>();
const expanded = ref(new Set<string>());
const loading = ref(false);
const error = ref('');
let request: AbortController | undefined;
onScopeDispose(() => request?.abort());

async function load(position?: string) {
  request?.abort();
  const controller = new AbortController();
  request = controller;
  loading.value = true;
  error.value = '';
  try {
    const result = await call({ oneofKind: 'query', query: { ...props.query, cursor: position } }, undefined, controller.signal);
    if (controller.signal.aborted) return;
    if (result.oneofKind !== 'page') throw Error('The resource level is unavailable.');
    items.value = result.page.items.filter(item => item.target && !props.ancestors.includes(key(item.target)));
    next.value = result.page.next;
    cursor.value = position;
    const visible = new Set(items.value.map(item => key(item.target)));
    for (const target of expanded.value) if (!visible.has(target)) expanded.value.delete(target);
  } catch (failure) {
    if (!controller.signal.aborted) error.value = String(failure);
  } finally {
    if (!controller.signal.aborted) loading.value = false;
  }
}
function select(item: model.Item) {
  if (!item.target) return;
  if (containers.has(item.kind) && ![K.L2_DOMAIN, K.SOFTWARE_PRODUCT].includes(item.kind)) emit('select', item);
  else emit('inspect', item.target);
}
function toggle(target: string) {
  if (!expanded.value.delete(target)) expanded.value.add(target);
}
// Continuations belong to the previous inventory revision. Reload the first
// page while retaining expansion for identities still present on that page.
watch(() => props.refresh, () => void load(), { immediate: true });
</script>

<template>
  <p v-if="loading" role="status">Loading resources…</p>
  <p v-else-if="error" role="alert">{{ error }} <button @click="load()">Reload level</button></p>
  <p v-else-if="!items.length && !next" class="hint">No resources at this level.</p>
  <ul v-if="!error && items.length">
    <li v-for="item in items" :key="key(item.target)">
      <button v-if="containers.has(item.kind) && ancestors.length < 12"
        :aria-label="`${expanded.has(key(item.target)) ? 'Collapse' : 'Expand'} ${identityName(item)}`"
        :aria-expanded="expanded.has(key(item.target))"
        @click="toggle(key(item.target))">{{ expanded.has(key(item.target)) ? '−' : '+' }}</button>
      <button :aria-current="key(item.target) === key(selected) ? 'true' : undefined"
        @click="select(item)">{{ identityName(item) }}</button>
      <small>{{ label(item.kind) }}</small>
      <NetworkTree v-if="expanded.has(key(item.target))" :key="key(item.target)"
        :query="{ ...query, focus: item.target }" :refresh="refresh" :selected="selected" :ancestors="[...ancestors, key(item.target)]"
        @select="emit('select', $event)" @inspect="emit('inspect', $event)" />
    </li>
  </ul>
  <div v-if="!error && (cursor || next)" class="actions">
    <button v-if="cursor" :disabled="loading" @click="load()">First resources</button>
    <button v-if="next" :disabled="loading" @click="load(next)">More resources</button>
  </div>
</template>
