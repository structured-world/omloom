<script setup lang="ts">
import { ref, onMounted } from 'vue';
import Dialog from './Dialog.vue';
import { call, model, AssetKind as K, display, label, type AssetRef } from './api';
const props = defineProps<{ item?: model.Item }>();
const emit = defineEmits<{ close: []; saved: [] }>();
const value = (field: string) => display(props.item?.fields.find(f => f.name === field));
const referenceId = (field: string) => value(field).split('/')[0] ?? '';
const items = ref<model.Item[]>([]); const error = ref(''); const name = ref(value('name')); const cidr = ref(value('cidr')); const space = ref(referenceId('address_space')); const link = ref(referenceId('link_scope')); const routing = ref(referenceId('routing_context')); const kind = ref(props.item?.kind ?? K.IP_ADDRESS);
const sharingValue = value('sharing');
const sharing = ref(sharingValue === 'SHARED_USE_VRRP' ? model.SharedUse.VRRP : sharingValue === 'SHARED_USE_ANYCAST' ? model.SharedUse.ANYCAST : model.SharedUse.EXCLUSIVE); const vrid = ref<number | undefined>(value('vrid') ? Number(value('vrid')) : undefined); const next = ref(false);
onMounted(async () => { try { let cursor: string | undefined; do {
  const r = await call({ oneofKind: 'query', query: model.Query.create({ kinds: [K.ADDRESS_SPACE, K.L2_DOMAIN, K.VRF], columns: ['name'], limit: 100, cursor }) });
  if (r.oneofKind !== 'page') throw Error('Unexpected response'); items.value.push(...r.page.items); cursor = r.page.next;
} while (cursor && items.value.length < 1024); } catch (e) { error.value = String(e); } });
function targetRef(id: string): AssetRef | undefined { return items.value.find(i => i.target?.assetId === id)?.target; }
async function save() {
  try {
    await call({ oneofKind: 'network', network: model.Network.create({ target: props.item?.target, expected: props.item?.revision, name: name.value, cidr: cidr.value, kind: kind.value, addressSpace: targetRef(space.value), link: targetRef(link.value), routing: targetRef(routing.value), sharing: sharing.value, vrid: sharing.value === model.SharedUse.VRRP ? vrid.value : undefined, nextAvailable: next.value }) }); emit('saved');
  } catch (e) { error.value = String(e); }
}
</script>
<template><Dialog title="Allocate network resource" @close="emit('close')"><form @submit.prevent="save"><p class="hint">Address space controls uniqueness. L2 and routing scopes are independent; matching names, CIDRs or VLAN IDs never merge them.</p>
<label>Name<input v-model="name" required maxlength="160"></label><label>Kind<select v-model="kind"><option :value="K.IP_ADDRESS">IP address</option><option :value="K.PREFIX">Prefix</option></select></label><label>Address / prefix<input v-model="cidr" required placeholder="192.0.2.1/24 or 2001:db8::1/64" maxlength="64"></label>
<label v-for="entry in [{ name: 'Address space', kind: K.ADDRESS_SPACE }, { name: 'L2 domain', kind: K.L2_DOMAIN }, { name: 'Routing context', kind: K.VRF }]" :key="entry.kind">{{ entry.name }}<select :value="entry.kind === K.ADDRESS_SPACE ? space : entry.kind === K.L2_DOMAIN ? link : routing" :required="entry.kind === K.ADDRESS_SPACE" @change="entry.kind === K.ADDRESS_SPACE ? space = ($event.target as HTMLSelectElement).value : entry.kind === K.L2_DOMAIN ? link = ($event.target as HTMLSelectElement).value : routing = ($event.target as HTMLSelectElement).value"><option value="">Select {{ entry.name.toLowerCase() }}</option><option v-for="i in items.filter(i => i.kind === entry.kind)" :key="i.target?.assetId" :value="i.target?.assetId">{{ display(i.fields.find(f => f.name === 'name')) }} · {{ i.target?.assetId.slice(-6) }}</option></select></label>
<label>Address use<select v-model="sharing"><option :value="model.SharedUse.EXCLUSIVE">Exclusive</option><option :value="model.SharedUse.VRRP">Explicit VRRP group</option><option :value="model.SharedUse.ANYCAST">Explicit anycast</option></select></label><label v-if="sharing === model.SharedUse.VRRP">VRID<input v-model.number="vrid" type="number" min="1" max="255" required></label><label class="check"><input v-model="next" type="checkbox">Atomically allocate next available host in the supplied prefix</label><p v-if="error" role="alert">{{ error }}</p><footer><button type="button" @click="emit('close')">Cancel</button><button class="primary">Allocate</button></footer></form></Dialog></template>
