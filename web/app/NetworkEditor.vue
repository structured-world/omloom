<script setup lang="ts">
import { ref, onMounted, computed, watch } from 'vue';
import Dialog from './Dialog.vue';
import AssetEditor from './AssetEditor.vue';
import { call, model, AssetKind as K, display, input, type AssetRef } from './api';
const props = defineProps<{ item?: model.Item; group?: model.Item; definitions: model.Definition[]; working: model.WorkingContext; allocations: model.Creation[]; view: model.View; reviewOnly?: boolean }>();
const parents = computed(() => [...new Map(props.allocations.filter(a => a.parent?.target).map(a => [a.parent!.target!.assetId,a.parent!])).values()]);
const parent = ref(parents.value.length === 1 ? parents.value[0]!.target!.assetId : '');
const container = computed(() => parents.value.find(p => p.target?.assetId === parent.value));
const availableKinds = computed(() => props.allocations.filter(a => (a.parent?.target?.assetId ?? '') === parent.value).map(a => a.kind));
const emit = defineEmits<{ close: []; saved: []; refresh: []; reviewed: [request: model.Network, working: model.WorkingContext] }>();
const creatingScope = ref(false);
async function scopeSaved() { creatingScope.value = false; await loadOptions(); emit('refresh'); }
const value = (field: string) => input(props.item?.fields.find(f => f.name === field));
const referenceId = (field: string) => value(field).split('/')[0] ?? '';
const items = ref<model.Item[]>([]); const error = ref(''); const name = ref(value('name')); const cidr = ref(value('cidr')); const space = ref(referenceId('address_space')); const link = ref(referenceId('link_scope')); const routing = ref(referenceId('routing_context')); const kind = ref(props.item?.kind ?? K.IP_ADDRESS);
watch(availableKinds, choices => { if (!props.item && choices.length && !choices.includes(kind.value)) kind.value = choices[0]!; }, { immediate: true });
const sharingValue = value('sharing');
const sharing = ref(sharingValue === 'SHARED_USE_VRRP' ? model.SharedUse.VRRP : sharingValue === 'SHARED_USE_ANYCAST' ? model.SharedUse.ANYCAST : model.SharedUse.EXCLUSIVE); const vrid = ref<number | undefined>(value('vrid') ? Number(value('vrid')) : undefined); const next = ref(false);
const vlanId = ref<number | undefined>(value('vlan_id') ? Number(value('vlan_id')) : undefined);
const groupMode = ref(props.group ? 'existing' : 'new');
const selectedGroup = ref(props.group?.target?.assetId ?? ''); const groupName = ref('');
const lookup = ref(''); const coverage = ref('');
const revision = ref<bigint>();
const matchingGroups = computed(() => items.value.filter(i => i.kind === K.ADDRESS_GROUP && input(i.fields.find(f => f.name === 'sharing')) === `SHARED_USE_${model.SharedUse[sharing.value]}`));
async function loadOptions() {
  try {
    error.value = ''; coverage.value = '';
    const retained = items.value.filter(i => [space.value, link.value, routing.value, selectedGroup.value].includes(i.target?.assetId ?? ''));
    const found: model.Item[] = []; let cursor: string | undefined;
    do {
      const r = await call({ oneofKind: 'query', query: model.Query.create({ roots: props.working.roots, kinds: [K.ADDRESS_SPACE, K.L2_DOMAIN, K.VRF, K.ADDRESS_GROUP], columns: ['name', 'sharing', 'vrid', 'link_scope'], search: lookup.value, limit: 100, cursor }) });
      if (r.oneofKind !== 'page') throw Error('Unexpected response'); found.push(...r.page.items); cursor = r.page.next; revision.value = r.page.revision;
    } while (cursor && found.length < 1000);
    items.value = [...new Map([...retained, ...found].map(i => [i.target?.assetId, i])).values()];
    if (cursor) coverage.value = 'Showing a bounded selection. Search for a scope or group name to narrow the options.';
  } catch (e) { error.value = String(e); }
}
onMounted(loadOptions);
function targetRef(id: string): AssetRef | undefined { return items.value.find(i => i.target?.assetId === id)?.target; }
async function save() {
  try {
    const vlan = kind.value === K.VLAN;
    const groupEdit = kind.value === K.ADDRESS_GROUP;
    const shared = kind.value === K.IP_ADDRESS && sharing.value !== model.SharedUse.EXCLUSIVE;
    const chosen = shared && groupMode.value === 'existing' ? items.value.find(i => i.kind === K.ADDRESS_GROUP && i.target?.assetId === selectedGroup.value) : undefined;
    if (shared && groupMode.value === 'existing' && (!chosen || input(chosen.fields.find(f => f.name === 'sharing')) !== `SHARED_USE_${model.SharedUse[sharing.value]}`)) throw Error('Select a group with the reviewed address mechanism.');
    const selectedVrrp = !!chosen && sharing.value === model.SharedUse.VRRP;
    const request = model.Network.create({
      target: props.item?.target, expected: props.item?.revision, name: name.value, kind: kind.value, parent: props.item ? undefined : container.value?.target,
      cidr: vlan || groupEdit ? '' : cidr.value, addressSpace: vlan || groupEdit ? undefined : targetRef(space.value),
      link: selectedVrrp ? undefined : targetRef(link.value), routing: vlan || groupEdit ? undefined : targetRef(routing.value),
      sharing: chosen ? model.SharedUse.UNSPECIFIED : vlan || kind.value === K.PREFIX ? model.SharedUse.EXCLUSIVE : sharing.value,
      vrid: !vlan && !selectedVrrp && sharing.value === model.SharedUse.VRRP ? vrid.value : undefined,
      nextAvailable: kind.value === K.IP_ADDRESS && !props.item && next.value, vlanId: vlan ? vlanId.value : undefined,
      group: chosen?.target, expectedGroup: chosen?.revision, groupName: shared && !chosen ? groupName.value : '',
      previousGroupRevision: props.group && props.group.target?.assetId !== chosen?.target?.assetId ? props.group.revision : undefined,
    });
    const working = model.WorkingContext.create({ ...props.working, expected: revision.value });
    if (props.reviewOnly) { emit('reviewed',request,working); return; }
    await call({ oneofKind: 'network', network: request }, working); emit('saved');
  } catch (e) { error.value = String(e); }
}
</script>
<template><Dialog :title="item ? 'Review network settings' : 'Allocate network resource'" @close="emit('close')">
  <form @submit.prevent="save">
    <p class="hint">Address space controls uniqueness. L2 and routing scopes are independent; matching names, CIDRs or VLAN IDs never merge them.</p>
    <label v-if="parents.length > 1">Allocation container<select v-model="parent" required><option value="">Select a container</option><option v-for="p in parents" :key="p.target?.assetId" :value="p.target?.assetId">{{ display(p.fields.find(f => f.name === 'name')) }}</option></select></label>
    <p v-else-if="container" class="hint">Within {{ display(container.fields.find(f => f.name === 'name')) }}</p>
    <label>Name<input v-model="name" required maxlength="160"></label>
    <label>Find scopes and groups<input v-model="lookup" maxlength="256" @keydown.enter.prevent="loadOptions"></label><button type="button" @click="loadOptions">Find matching options</button><p v-if="coverage" class="hint">{{ coverage }}</p>
    <label for="network-kind">Kind</label>
    <select id="network-kind" v-model="kind" :disabled="!!item"><option v-if="item || availableKinds.includes(K.IP_ADDRESS)" :value="K.IP_ADDRESS">IP address</option><option v-if="item || availableKinds.includes(K.PREFIX)" :value="K.PREFIX">Prefix</option><option v-if="item || availableKinds.includes(K.VLAN)" :value="K.VLAN">Scoped VLAN</option><option v-if="item?.kind === K.ADDRESS_GROUP" :value="K.ADDRESS_GROUP">Shared address group</option></select>
    <label v-if="[K.IP_ADDRESS, K.PREFIX].includes(kind)">Address / prefix<input v-model="cidr" required placeholder="192.0.2.1/24 or 2001:db8::1/64" maxlength="64"></label>
    <label v-if="kind === K.VLAN">VLAN ID<input v-model.number="vlanId" required type="number" min="1" max="4094"></label>
    <template v-for="entry in [{ name: 'Address space', kind: K.ADDRESS_SPACE }, { name: 'L2 domain', kind: K.L2_DOMAIN }, { name: 'Routing context', kind: K.VRF }]" :key="entry.kind">
      <template v-if="!(container && (entry.kind === K.ADDRESS_SPACE || (container.kind === K.L2_DOMAIN && entry.kind === K.L2_DOMAIN))) && (kind === K.ADDRESS_GROUP ? entry.kind === K.L2_DOMAIN && sharing === model.SharedUse.VRRP : kind !== K.VLAN || entry.kind === K.L2_DOMAIN) && !(kind === K.IP_ADDRESS && sharing === model.SharedUse.VRRP && groupMode === 'existing' && entry.kind === K.L2_DOMAIN)">
        <label :for="`scope-${entry.kind}`">{{ entry.name }}</label>
        <select :id="`scope-${entry.kind}`" :value="entry.kind === K.ADDRESS_SPACE ? space : entry.kind === K.L2_DOMAIN ? link : routing" :required="entry.kind === K.ADDRESS_SPACE || (entry.kind === K.L2_DOMAIN && (kind === K.VLAN || sharing === model.SharedUse.VRRP))" @change="entry.kind === K.ADDRESS_SPACE ? space = ($event.target as HTMLSelectElement).value : entry.kind === K.L2_DOMAIN ? link = ($event.target as HTMLSelectElement).value : routing = ($event.target as HTMLSelectElement).value">
          <option value="">Select {{ entry.name.toLowerCase() }}</option><option v-for="i in items.filter(i => i.kind === entry.kind)" :key="i.target?.assetId" :value="i.target?.assetId">{{ display(i.fields.find(f => f.name === 'name')) }} · {{ i.target?.assetId.slice(-6) }}</option>
        </select>
        <button v-if="entry.kind === K.ADDRESS_SPACE" type="button" @click="creatingScope = true">Create address space</button>
      </template>
    </template>
    <template v-if="[K.IP_ADDRESS, K.ADDRESS_GROUP].includes(kind)">
      <label for="address-use">Address use</label><select id="address-use" v-model="sharing"><option :value="model.SharedUse.EXCLUSIVE">Exclusive</option><option :value="model.SharedUse.VRRP">Explicit VRRP group</option><option :value="model.SharedUse.ANYCAST">Explicit anycast</option></select>
      <template v-if="kind === K.IP_ADDRESS && sharing !== model.SharedUse.EXCLUSIVE">
        <label for="group-mode">Group selection</label><select id="group-mode" v-model="groupMode"><option value="new">Create a named group</option><option value="existing">Use an existing group</option></select>
        <label v-if="groupMode === 'new'">New group name<input v-model="groupName" required maxlength="160"></label>
        <template v-else><label for="network-group">Shared address group</label><select id="network-group" v-model="selectedGroup" required><option value="">Select permitted group</option><option v-for="g in matchingGroups" :key="g.target?.assetId" :value="g.target?.assetId">{{ display(g.fields.find(f => f.name === 'name')) }} · {{ g.target?.assetId.slice(-8) }}</option></select><p class="hint">Membership is shared by all addresses of this group. Edits check its independent revision.</p></template>
      </template>
      <label v-if="sharing === model.SharedUse.VRRP && (kind === K.ADDRESS_GROUP || groupMode === 'new')">VRID<input v-model.number="vrid" type="number" min="1" max="255" required></label>
      <p v-if="item && sharing === model.SharedUse.VRRP" class="hint">Review the conversion: an existing exclusive participant remains the address owner with intended priority 255. No active gateway is inferred.</p>
      <p v-if="group && (sharing === model.SharedUse.EXCLUSIVE || selectedGroup !== group.target?.assetId || groupMode === 'new')" class="hint">Leaving the current group keeps its participants and other addresses intact. This address remains allocated.</p>
      <label v-if="!item && kind === K.IP_ADDRESS" class="check"><input v-model="next" type="checkbox">Atomically allocate next available host in the supplied prefix</label>
    </template>
    <p v-if="reviewOnly" class="hint">This step only prepares the allocation. It is committed together with the address mapping after your final review.</p>
    <p v-if="error" role="alert">{{ error }}</p><footer><button type="button" @click="emit('close')">Cancel</button><button class="primary">{{ reviewOnly ? 'Review allocation' : item ? 'Apply reviewed settings' : 'Allocate' }}</button></footer>
  </form>
</Dialog><AssetEditor v-if="creatingScope" :initial-kind="K.ADDRESS_SPACE" :definitions="definitions" :working="working" :view="view" @close="creatingScope = false" @saved="scopeSaved" /></template>
