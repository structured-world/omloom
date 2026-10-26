<script setup lang="ts">
import { computed, onMounted, ref } from 'vue';
import Dialog from './Dialog.vue';
import NetworkEditor from './NetworkEditor.vue';
import { call, model, key, input, display, AssetKind as K } from './api';
const props = defineProps<{ item: model.Item; working: model.WorkingContext; definitions: model.Definition[] }>();
const emit = defineEmits<{ close: []; saved: [] }>();
const entered = ref(false); const reviewing = ref(false); const allocating = ref(false);
const roots = ref(props.working.roots); const revision = ref<bigint>();
const actions = ref(model.Actions.create()); const items = ref<model.Item[]>([]); const interfaces = ref<model.Item[]>([]);
const search = ref(input(props.item.fields.find(f => f.name === 'address'))); const next = ref<string>();
const interfaceSearch = ref(''); const interfaceNext = ref<string>();
const selected = ref(''); const interfaceId = ref(''); const allocation = ref<model.Network>();
const preparedUse = ref<model.SharedUse>(); const priority = ref<number>();
const remove = ref(false); const error = ref('');
const candidate = computed(() => items.value.find(i => key(i.target) === selected.value));
const vrrp = computed(() => allocation.value ? preparedUse.value === model.SharedUse.VRRP : input(candidate.value?.fields.find(f => f.name === 'sharing')) === `SHARED_USE_${model.SharedUse[model.SharedUse.VRRP]}`);
const needsPriority = computed(() => !remove.value && configured && !!interfaceId.value && vrrp.value);
const allocationActions = computed(() => actions.value.allocations.filter(a => a.kind === K.IP_ADDRESS));
const configured = props.item.fields.some(f => f.name === 'meaning' && input(f) === `ADDRESS_MEANING_${model.AddressMeaning[model.AddressMeaning.CONFIGURED]}`);
const label = (item: model.Item) => display(item.fields.find(f => f.name === 'name')) || item.target!.assetId.slice(-8);
async function load(cursor?: string) {
  error.value = '';
  try {
    const page = await call({ oneofKind: 'query', query: model.Query.create({ roots: roots.value, kinds: [K.IP_ADDRESS], columns: ['name','cidr','address_space','link_scope','sharing'], search: search.value, cursor, limit: 50 }) });
    if (page.oneofKind !== 'page') throw Error('Allocation selection is unavailable');
    items.value = page.page.items; next.value = page.page.next; revision.value = page.page.revision;
    const prepared = await call({ oneofKind: 'prepare', prepare: model.Prepare.create({ view: model.View.NETWORK }) }, model.WorkingContext.create({ roots: roots.value }));
    if (prepared.oneofKind !== 'actions') throw Error('Allocation actions are unavailable');
    actions.value = prepared.actions;
    entered.value = true;
  } catch (e) { error.value = String(e); }
}
async function review() {
  error.value = '';
  try {
    if (!remove.value && !allocation.value && !candidate.value) throw Error('Select an allocation');
    await loadInterfaces();
    reviewing.value = true;
  } catch (e) { error.value = String(e); }
}
async function loadInterfaces(cursor?: string) {
  const page = await call({ oneofKind: 'query', query: model.Query.create({ roots: roots.value, kinds: [K.INTERFACE], columns: ['name'], search: interfaceSearch.value, cursor, limit: 100 }) });
  if (page.oneofKind !== 'page') throw Error('Assignment context is unavailable');
  interfaces.value = page.page.items; interfaceNext.value = page.page.next; revision.value = page.page.revision; interfaceId.value = '';
}
async function save() {
  error.value = '';
  try {
    if (needsPriority.value && (priority.value === undefined || !Number.isInteger(priority.value) || priority.value < 1 || priority.value > 255)) throw Error('Enter an intended VRRP priority from 1 to 255.');
    await call({ oneofKind: 'address', address: model.Address.create({
      target: props.item.target, expected: props.item.revision,
      binding: remove.value || allocation.value ? undefined : candidate.value?.target,
      allocation: remove.value ? undefined : allocation.value, clearBinding: remove.value,
      interface: interfaceId.value ? interfaces.value.find(i => key(i.target) === interfaceId.value)?.target : undefined,
      priority: needsPriority.value ? priority.value : undefined,
    }) }, model.WorkingContext.create({ roots: roots.value, expected: revision.value }));
    emit('saved');
  } catch (e) { error.value = String(e); }
}
function reviewed(request: model.Network, working: model.WorkingContext, sharing: model.SharedUse) { allocation.value = request; preparedUse.value = sharing; revision.value = working.expected; allocating.value = false; void review(); }
onMounted(() => { if (props.working.focus) error.value = 'Select an enclosing context before resolving the address'; });
</script>
<template><Dialog title="Resolve address in broader scope" @close="emit('close')">
<p class="context-chip">Known fact: {{ label(item) }}</p>
<p class="hint">Recording an address did not allocate or assign it. Review a real namespace mapping. Conflicts preserve the original fact.</p>
<template v-if="!entered"><p>This opens the enclosing permitted inventory to select the canonical allocation.</p><button class="primary" @click="load()">Choose allocation context</button></template>
<template v-else-if="!reviewing">
<button v-if="roots.length" @click="roots = []; load()">Choose across permitted inventory</button>
<form @submit.prevent="load()"><label>Find canonical address<input v-model="search" maxlength="256"></label><button>Find allocations</button></form>
<label for="canonical-allocation">Canonical allocation</label><select id="canonical-allocation" v-model="selected" @change="allocation = undefined; remove = false"><option value="">Select an existing allocation</option><option v-for="i in items" :key="key(i.target)" :value="key(i.target)">{{ label(i) }} · {{ input(i.fields.find(f => f.name === 'cidr')) }} · {{ input(i.fields.find(f => f.name === 'address_space')) }}</option></select>
<button v-if="next" @click="load(next)">Next allocations</button>
<button v-if="allocationActions.length" @click="allocating = true">Prepare new allocation</button>
<button @click="remove = true; allocation = undefined; interfaceId = ''; review()">Review removing mapping</button>
<footer><button class="primary" :disabled="!candidate" @click="review">Review address mapping</button></footer>
</template>
<template v-else>
<p v-if="remove">Remove only the reference mapping. The canonical allocation and its assignments remain unchanged.</p>
<template v-else><p>{{ allocation ? 'New allocation: '+allocation.name+' · '+allocation.cidr : 'Canonical allocation: '+(candidate ? label(candidate) : '') }}</p><p v-if="allocation">The allocation and mapping will commit together or neither will be saved.</p>
<template v-if="configured"><label>Find assignment interface<input v-model="interfaceSearch" maxlength="256"></label><button @click="loadInterfaces().catch(e => error = String(e))">Find interfaces</button><label for="binding-interface">Intended assignment</label><select id="binding-interface" v-model="interfaceId"><option value="">Map knowledge only, do not assign</option><option v-for="i in interfaces" :key="key(i.target)" :value="key(i.target)">{{ label(i) }}</option></select><button v-if="interfaceNext" @click="loadInterfaces(interfaceNext).catch(e => error = String(e))">Next interfaces</button></template></template>
<label v-if="needsPriority">Intended priority<input v-model.number="priority" type="number" min="1" max="255" required></label>
<footer><button @click="reviewing = false">Back</button><button class="primary" @click="save">{{ remove ? 'Remove reviewed mapping' : 'Apply reviewed mapping' }}</button></footer>
</template>
<p v-if="error" role="alert">{{ error }}</p><footer><button @click="emit('close')">Return to resource</button></footer>
</Dialog><NetworkEditor v-if="allocating" :definitions="definitions" :working="model.WorkingContext.create({ roots })" :allocations="allocationActions" :view="model.View.NETWORK" review-only @close="allocating = false" @reviewed="reviewed" /></template>
