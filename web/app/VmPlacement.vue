<script setup lang="ts">
import { onMounted, ref } from 'vue';
import Dialog from './Dialog.vue';
import { call, model, key, display, text, AssetKind, RelationKind, type AssetRef } from './api';
const props = defineProps<{ target: AssetRef; working: model.WorkingContext }>();
const emit = defineEmits<{ close: []; saved: [vm: AssetRef, host: AssetRef] }>();
const bounds = ref<AssetRef[]>(props.working.roots);
const review = ref<model.VmPlacementReview>();
const items = ref<model.Item[]>([]); const next = ref<string>(); const search = ref('');
const contexts = ref<model.ItemContext[]>([]);
const busy = ref(false); const error = ref(''); const step = ref<'choose' | 'review'>('choose');
function name(item: model.Item | undefined) {
  if (!item) return 'No accepted placement';
  const value = display(item.fields.find(f => f.name === 'name'));
  return value || `Unnamed resource (${item.target?.assetId.slice(-8)})`;
}
function context(expected?: bigint) { return model.WorkingContext.create({ roots: bounds.value, expected }); }
function hostContext(item: model.Item | undefined, values: model.ItemContext[]) {
  const value = values.find(c => key(c.target) === key(item?.target));
  if (!value) return '';
  return [...value.owners.map(o => o.name || o.target?.assetId), ...value.addresses].filter(Boolean).join(' / ');
}
async function prepare(destination?: AssetRef) {
  const response = await call({ oneofKind: 'prepareVmPlacement', prepareVmPlacement: { vm: props.target, destination } }, context());
  if (response.oneofKind !== 'vmPlacementReview') throw Error('Placement review is unavailable');
  review.value = response.vmPlacementReview;
}
async function candidates(cursor?: string) {
  const requestedSearch = search.value;
  const response = await call({ oneofKind: 'query', query: model.Query.create({
    view: model.View.VIRTUALIZATION, roots: bounds.value, kinds: [AssetKind.SOFTWARE_INSTALLATION],
    columns: ['name'], search: requestedSearch, cursor, limit: 20, includeContext: true,
    predicates: [{ field: 'virtualization_role', value: text('virtualization_role', `VIRTUALIZATION_ROLE_${model.VirtualizationRole[model.VirtualizationRole.HYPERVISOR]}`) }],
  }) });
  if (response.oneofKind !== 'page') throw Error('Hypervisor selection is unavailable');
  if (search.value !== requestedSearch) return;
  items.value = response.page.items; next.value = response.page.next; contexts.value = response.page.contexts;
}
async function load() {
  busy.value = true; error.value = '';
  try { await prepare(); await candidates(); step.value = 'choose'; }
  catch (e) { error.value = String(e); } finally { busy.value = false; }
}
async function find(cursor?: string) {
  busy.value = true; error.value = '';
  try { await candidates(cursor); } catch (e) { error.value = String(e); } finally { busy.value = false; }
}
async function choose(destination: AssetRef | undefined) {
  if (!destination || busy.value) return;
  busy.value = true; error.value = '';
  try { await prepare(destination); step.value = 'review'; }
  catch (e) { error.value = String(e); } finally { busy.value = false; }
}
function drag(event: DragEvent) { event.dataTransfer?.setData('application/x-omloom-vm', key(props.target)); }
function drop(event: DragEvent, item: model.Item) {
  if (event.dataTransfer?.getData('application/x-omloom-vm') === key(props.target)) void choose(item.target);
}
async function save() {
  const value = review.value; const destination = value?.destination?.target;
  if (!value || !destination) return;
  busy.value = true; error.value = '';
  try {
    const relation = value.current ? { ...value.current, to: destination } : { from: props.target, to: destination, kind: RelationKind.RUNS_ON };
    const response = await call({ oneofKind: 'relation', relation: model.Relationship.create({ relation }) }, context(value.revision));
    if (response.oneofKind !== 'mutation') throw Error('Placement was not accepted');
    emit('saved', props.target, destination);
  } catch (e) { error.value = String(e); } finally { busy.value = false; }
}
onMounted(load);
</script>
<template><Dialog title="Record VM placement" @close="emit('close')">
<div class="placement-body">
<p>Record a migration already completed elsewhere. This does not move the VM on a hypervisor or change its IP addresses.</p>
<p v-if="error" class="error" role="alert">{{ error }} <button :disabled="busy" @click="load">Refresh placement</button></p>
<p v-if="busy" role="status">Checking current placement…</p>
<template v-if="review">
<article class="placement-guest" :draggable="step === 'choose' && !busy" @dragstart="drag" aria-label="VM to relocate">
<strong>{{ name(review.vm) }}</strong><template v-if="step === 'choose'"><p>Current host: {{ name(review.currentHost) }}</p><small>{{ hostContext(review.currentHost,review.contexts) }}</small></template>
<small v-if="step === 'choose'">Drag this VM onto a destination, or use its Choose button.</small></article>
<template v-if="step === 'choose'">
<form class="actions" @submit.prevent="find()"><label>Find hypervisor<input v-model="search" maxlength="200" type="search" @input="next = undefined"></label><button :disabled="busy">Search</button></form>
<p>Only enabled hypervisors in the selected enclosing context are offered.</p>
<div class="placement-destinations" aria-label="Destination hypervisors"><article v-for="item in items" :key="key(item.target)" :aria-label="`Destination ${name(item)}`" class="placement-destination" @dragover.prevent @drop.prevent="drop($event,item)">
<strong>{{ name(item) }}</strong><small>{{ hostContext(item,contexts) }}</small><small class="mono">{{ item.target?.assetId }}</small><small v-if="key(item.target) === key(review.currentHost?.target)">Current host</small>
<button :disabled="busy || key(item.target) === key(review.currentHost?.target)" @click="choose(item.target)">Choose {{ name(item) }}</button>
</article></div>
<p v-if="!items.length">No eligible hypervisors are visible in this selection.</p>
<div class="actions"><button v-if="next" :disabled="busy" @click="find(next)">Next hypervisors</button><button v-if="bounds.length" :disabled="busy" @click="bounds=[]; load()">Choose from all permitted infrastructure</button></div>
</template>
<template v-else>
<div class="placement-comparison"><section><h3>From</h3><strong>{{ name(review.currentHost) }}</strong><small>{{ hostContext(review.currentHost,review.contexts) }}</small><small class="mono">{{ review.currentHost?.target?.assetId }}</small></section><section><h3>To</h3><strong>{{ name(review.destination) }}</strong><small>{{ hostContext(review.destination,review.contexts) }}</small><small class="mono">{{ review.destination?.target?.assetId }}</small></section></div>
<p>Keep this VM, its OS installations, interfaces, services and current addresses. Existing grants and direct ownership do not change.</p>
<section v-if="review.invalidatedInheritance.length" aria-label="Inheritance requiring review"><h3>Review inheritance after this move</h3><p>These explicit old-placement bindings will become unavailable. The new host does not become their administrative source.</p>
<ul><li v-for="facet in review.invalidatedInheritance" :key="facet.facet">{{ model.ResponsibilityFacet[facet.facet]?.toLowerCase().replaceAll('_',' ') }}: <template v-if="facet.party">{{ facet.party.name || facet.party.subject }}</template><template v-else>{{ model.ResponsibilityStatus[facet.status]?.toLowerCase().replaceAll('_',' ') }}</template></li></ul></section>
<p>Service binds, routes, network and storage membership are not inferred from the new host. Any new IPs require their own current observation.</p>
<div class="actions placement-actions"><button :disabled="busy" @click="step='choose'">Back to destinations</button><button :disabled="busy" @click="choose(review.destination?.target)">Refresh review</button><button class="primary" :disabled="busy || !review.destination || key(review.currentHost?.target) === key(review.destination.target)" @click="save">Record and view VM</button></div>
</template></template>
</div>
</Dialog></template>
<style scoped>
.placement-body { padding:0 1.5rem 1.5rem; }
.placement-body small { display:block; margin-top:.35rem; }
.placement-comparison { display:grid; grid-template-columns:repeat(2,minmax(0,1fr)); gap:1rem; margin-block:1rem; }
.placement-comparison section { padding:1rem; border:1px solid var(--line,#c8d5db); border-radius:8px; overflow-wrap:anywhere; }
.placement-comparison h3 { margin:0 0 .5rem; font-size:.85rem; }
.placement-comparison .mono { font-size:.75rem; }
.placement-actions { justify-content:flex-end; margin-top:1.5rem; }
.placement-guest,.placement-destination { border:1px solid var(--line,#c8d5db); border-radius:8px; padding:1rem; margin-block:1rem; overflow-wrap:anywhere; }
.placement-guest[draggable=true] { cursor:grab; background:var(--surface,#f0f7f7); }
.placement-destinations { display:grid; grid-template-columns:repeat(auto-fit,minmax(min(100%,14rem),1fr)); gap:.75rem; }
.placement-destination { display:flex; flex-direction:column; gap:.75rem; margin:0; }
.placement-destination:hover { border-color:currentColor; }
form label { flex:1; min-width:0; } input { width:100%; }
@media(max-width:600px) { .placement-body { padding:0 1rem 1rem; } .placement-comparison { grid-template-columns:1fr; } .placement-actions .primary { width:100%; } }
</style>
