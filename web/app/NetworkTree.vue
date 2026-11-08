<script setup lang="ts">
import { onScopeDispose, ref } from 'vue';
import { AssetKind as K, call, identityName, key, label, model, type AssetRef } from './api';

const props = defineProps<{ query: model.Query; selected?: AssetRef; ancestors: string[] }>();
const emit = defineEmits<{ select: [target: AssetRef]; inspect: [target: AssetRef] }>();
const containers = new Set([K.SITE, K.LOCATION, K.RACK, K.DEVICE, K.VIRTUAL_MACHINE, K.SOFTWARE_INSTALLATION,
  K.SERVICE, K.WORKLOAD, K.CONTAINER, K.INTERFACE, K.PORT, K.PHYSICAL_COMPONENT, K.PHYSICAL_POSITION,
  K.ADDRESS_SPACE, K.PREFIX, K.L2_DOMAIN]);
const items = ref<model.Item[]>([]);
const next = ref<string>();
const cursor = ref<string>();
const expanded = ref('');
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
    if (!items.value.some(item => key(item.target) === expanded.value)) expanded.value = '';
  } catch (failure) {
    if (!controller.signal.aborted) error.value = String(failure);
  } finally {
    if (!controller.signal.aborted) loading.value = false;
  }
}
function select(item: model.Item) {
  if (!item.target) return;
  if (containers.has(item.kind)) emit('select', item.target);
  else emit('inspect', item.target);
}
void load();
</script>

<template>
  <p v-if="loading" role="status">Loading resources…</p>
  <p v-else-if="error" role="alert">{{ error }} <button @click="load()">Reload level</button></p>
  <p v-else-if="!items.length && !next" class="hint">No resources at this level.</p>
  <ul v-if="!error && items.length">
    <li v-for="item in items" :key="key(item.target)">
      <button v-if="containers.has(item.kind) && ancestors.length < 12"
        :aria-label="`${expanded === key(item.target) ? 'Collapse' : 'Expand'} ${identityName(item)}`"
        :aria-expanded="expanded === key(item.target)"
        @click="expanded = expanded === key(item.target) ? '' : key(item.target)">{{ expanded === key(item.target) ? '−' : '+' }}</button>
      <button :aria-current="key(item.target) === key(selected) ? 'true' : undefined"
        @click="select(item)">{{ identityName(item) }}</button>
      <small>{{ label(item.kind) }}</small>
      <NetworkTree v-if="expanded === key(item.target)" :key="expanded"
        :query="{ ...query, focus: item.target }" :selected="selected" :ancestors="[...ancestors, expanded]"
        @select="emit('select', $event)" @inspect="emit('inspect', $event)" />
    </li>
  </ul>
  <div v-if="!error && (cursor || next)" class="actions">
    <button v-if="cursor" :disabled="loading" @click="load()">First resources</button>
    <button v-if="next" :disabled="loading" @click="load(next)">More resources</button>
  </div>
</template>
