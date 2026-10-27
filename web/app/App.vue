<script setup lang="ts">
import HostNetwork from './HostNetwork.vue';
import CategoryNav from './CategoryNav.vue';
import CurrentAddresses from './CurrentAddresses.vue';
import { ref, computed, watch, onMounted, nextTick } from 'vue';
import { useRoute, useRouter } from 'vue-router';
import type { LocationQuery } from 'vue-router';
import AssetEditor from './AssetEditor.vue'; import SchemaEditor from './SchemaEditor.vue'; import ImportEditor from './ImportEditor.vue'; import ViewEditor from './ViewEditor.vue'; import NetworkEditor from './NetworkEditor.vue';
import MemberEditor from './MemberEditor.vue';
import FilterEditor from './FilterEditor.vue';
import BulkEditor from './BulkEditor.vue';
import RelationEditor from './RelationEditor.vue';
import AddressBinding from './AddressBinding.vue';
import SelectionEditor from './SelectionEditor.vue';
import { call, model, label, display, key, kinds, AssetKind as K, RelationKind, text, type AssetRef } from './api';
const router = useRouter(); const route = useRoute();
const identity = ref<model.Identity>(); const definitions = ref<model.Definition[]>([]); const views = ref<model.SavedView[]>([]);
const page = ref(model.Page.create()); const detail = ref<model.Detail>(); const loading = ref(false); const error = ref(''); const notice = ref('');
const preparedActions = ref(model.Actions.create());
const actions = computed(() => loading.value ? model.Actions.create() : preparedActions.value);
const editor = ref<'' | 'asset' | 'schema' | 'import' | 'view' | 'network' | 'bulk' | 'relation' | 'member' | 'filter' | 'kinds' | 'columns' | 'binding'>(''); const editing = ref<model.Item>();
const initialKind = computed(() => query.value.kinds.length===1 ? query.value.kinds[0] as K : ({ [model.View.OVERVIEW]: K.DEVICE, [model.View.NETWORK]: K.ADDRESS_REFERENCE, [model.View.HARDWARE]: K.DEVICE, [model.View.VIRTUALIZATION]: K.VIRTUAL_MACHINE, [model.View.SERVICES]: K.SERVICE })[query.value.view]);
const kindOptions = kinds.map(k => ({ id: String(k), name: label(k) }));
const columnOptions = computed(() => {
  const options = [{ id: 'name', name: 'Name' }, { id: 'description', name: 'Description' }, { id: 'serial', name: 'Serial number' }, ...definitions.value.filter(d => !d.retired)];
  for (const id of query.value.columns) if (!options.some(o => o.id === id)) options.push({ id, name: `${id} (unavailable)` });
  return options;
});
let listScroll = 0;
let listQuery = '';
let listFocus: HTMLElement | undefined;
let listSelection: string[] = [];
let returnQuery: LocationQuery | undefined;
const detailHeading = ref<HTMLElement>();
const moreActions = ref(false);
const selected = ref<string[]>([]); const search = ref('');
const viewSearch = ref('');
const matchingViews = computed(() => views.value.filter(v => v.name.toLocaleLowerCase().includes(viewSearch.value.toLocaleLowerCase())));
const presentations = [{ id: model.View.OVERVIEW, name: 'Overview', symbol: '▦' }, { id: model.View.NETWORK, name: 'Network', symbol: '⇄' }, { id: model.View.HARDWARE, name: 'Hardware', symbol: '▤' }, { id: model.View.VIRTUALIZATION, name: 'Virtualization', symbol: '◫' }, { id: model.View.SERVICES, name: 'Services', symbol: '◇' }];
function parameters(name: string): string[] {
  const value = route.query[name];
  if (value === undefined) return [];
  return (Array.isArray(value) ? value : [value]).map(String);
}
function roots(): AssetRef[] {
  const ids = parameters('scope'); const incarnations = parameters('scopeInc');
  if (ids.length !== incarnations.length) return [{ assetId: '', incarnation: '' }];
  return ids.map((assetId, index) => ({ assetId, incarnation: incarnations[index]! }));
}
const query = computed(() => model.Query.create({
  view: Number(route.query.view ?? 0), search: String((route.query.asset ? route.query.localSearch : route.query.search) ?? ''),
  kinds: parameters(route.query.asset ? 'localKind' : 'kind').map(Number), columns: String(route.query.columns ?? 'name,description').split(','),
  sort: String(route.query.sort ?? 'name'), descending: route.query.desc === '1', unplaced: !route.query.asset && route.query.unplaced === '1', limit: 25,
  cursor: typeof (route.query.asset ? route.query.localCursor : route.query.cursor) === 'string' ? String(route.query.asset ? route.query.localCursor : route.query.cursor) : undefined,
  roots: roots(),
  predicates: parsePredicates(route.query.asset ? 'localFilter' : 'filter'),
  focus: route.query.asset && route.query.incarnation ? { assetId: String(route.query.asset), incarnation: String(route.query.incarnation) } : undefined,
  includeContext: true, omitHostLocal: true,
  levelOnly: !String((route.query.asset ? route.query.localSearch : route.query.search) ?? '').trim()
    && !route.query[route.query.asset ? 'localFilter' : 'filter'] && route.query.unplaced !== '1',
}));
const working = computed(() => model.WorkingContext.create({ roots: query.value.roots, focus: query.value.focus, expected: page.value.revision }));
const enclosing = computed(() => model.WorkingContext.create({ roots: query.value.roots, expected: page.value.revision }));
function parsePredicates(parameter: string): model.Predicate[] {
  if (!route.query[parameter]) return [];
  try { const json = JSON.parse(String(route.query[parameter])); return model.Query.fromJson({ predicates: json }).predicates; } catch { return [{ field: 'unavailable', value: text('unavailable', '') }]; }
}
const activeView = computed(() => views.value.find(v => v.id === route.query.saved));
const currentTitle = computed(() => presentations.find(v => v.id === query.value.view)?.name ?? 'Inventory');
const initialEmpty = computed(()=>query.value.view===model.View.OVERVIEW && !query.value.focus && !query.value.roots.length && !query.value.search && !query.value.kinds.length && !query.value.predicates.length && !query.value.unplaced);
const columnLabels = computed(() => query.value.columns.map(id => ({ id, name: definitions.value.find(d => d.id === id)?.name ?? id.replaceAll('_', ' ') })));
const selectedItems = computed(() => page.value.items.filter(i => selected.value.includes(key(i.target))));
const rowContexts = computed(() => new Map(page.value.contexts.map(c => [key(c.target), c])));
const grouping = computed(() => !query.value.focus);
function contextPath(item: model.Item) {
  const context=rowContexts.value.get(key(item.target));
  if (context?.alternativeOwners.length) return `Shared: ${context.alternativeOwners.map(o=>o.name || label(o.kind)).join(' · ')}`;
  return context?.owners.map(o => o.name || label(o.kind)).join(' / ') || '';
}
const rowGroups = computed(() => {
  const groups = new Map<string, { title: string; owner?: AssetRef; items: model.Item[] }>();
  for (const item of page.value.items) {
    const context = rowContexts.value.get(key(item.target));
    const title = grouping.value ? contextPath(item) : '';
    const owner = context?.owners.at(-1)?.target;
    const groupKey = grouping.value ? [...context?.owners ?? [],...context?.alternativeOwners ?? []].map(o => key(o.target)).join('/') || 'roots' : 'local';
    let group = groups.get(groupKey);
    if (!group) { group = { title, owner, items: [] }; groups.set(groupKey, group); }
    group.items.push(item);
  }
  return [...groups.values()];
});
function rowAddresses(item: model.Item) {
  if (![K.SERVICE, K.INTERFACE].includes(item.kind)) return '';
  const context = rowContexts.value.get(key(item.target));
  if (!context) return '';
  if (item.kind===K.SERVICE && context.endpoints.length) return `Endpoints: ${context.endpoints.map(e=>`${e.bind.includes(':')?'['+e.bind+']':e.bind}:${e.port}/${e.protocol}${e.bind==='*'?' ('+e.family+')':''}`).join(', ')}`;
  if (context.addresses.length) return `Address: ${context.addresses.join(', ')}`;
  return [K.SERVICE, K.INTERFACE].includes(item.kind) ? 'Address not known' : '';
}
const detailName = computed(() => display(detail.value?.item?.fields.find(f => f.name === 'name')));
let generation = 0;
let loadedQuery = '';
async function load() {
  const turn = ++generation; loading.value = true; error.value = '';
  preparedActions.value = model.Actions.create();
  const signature = JSON.stringify(model.Query.toJson(query.value));
  if (signature !== loadedQuery) { selected.value = []; search.value = query.value.search; loadedQuery = signature; }
  try {
    const result = await call({ oneofKind: 'query', query: query.value });
    if (turn !== generation) return;
    if (result.oneofKind !== 'page') throw Error('Unexpected inventory response');
    page.value = result.page;
    const prepared = await call({ oneofKind: 'prepare', prepare: model.Prepare.create({ view: query.value.view }) }, working.value).catch(e=>{ if(turn===generation) error.value=String(e); return undefined; });
    if (turn !== generation) return;
    if (prepared?.oneofKind === 'actions') preparedActions.value = prepared.actions;
    selected.value = selected.value.filter(id => result.page.items.some(item => key(item.target) === id));
    const next = route.query.asset && route.query.incarnation ? await read({ assetId: String(route.query.asset), incarnation: String(route.query.incarnation) }).catch(e=>{if(turn===generation)error.value=String(e);return undefined;}) : undefined;
    if (turn === generation) detail.value = next;
  } catch (e) { if (turn === generation) { page.value = model.Page.create(); detail.value = undefined; error.value = String(e); } } finally { if (turn === generation) loading.value = false; }
}
async function metadata() {
  const [i, d, v] = await Promise.all([call({ oneofKind: 'identity', identity: {} }), call({ oneofKind: 'definitions', definitions: {} }), call({ oneofKind: 'views', views: {} })]);
  if (i.oneofKind === 'identity') identity.value = i.identity; if (d.oneofKind === 'definitions') definitions.value = d.definitions.items; if (v.oneofKind === 'views') views.value = v.views.items;
}
async function read(target: AssetRef, relationsCursor?: string) {
  const result = await call({ oneofKind: 'read', read: { target, relationsCursor } }, model.WorkingContext.create({ roots: query.value.roots, focus: target }));
  if (result.oneofKind !== 'detail') throw Error('Unexpected resource response');
  return result.detail;
}
async function nextRelations() {
  const current=detail.value; const target=current?.item?.target;
  if (!target || !current.relationsNext) return;
  try {
    const next=await read(target,current.relationsNext);
    if (detail.value===current) detail.value={...current,relations:next.relations,related:next.related,relationsNext:next.relationsNext};
  } catch(e) {error.value=String(e);}
}
async function change(values: Record<string, string | string[] | undefined>) {
  const focused = 'asset' in values ? values.asset : route.query.asset;
  if (focused) {
    for (const [field, local] of [['search','localSearch'],['kind','localKind'],['filter','localFilter'],['cursor','localCursor']] as const) {
      if (field in values) { values[local] = values[field]; delete values[field]; }
    }
    await router.push({ path: '/', query: { ...route.query, localCursor: undefined, ...values } });
  } else await router.push({ path: '/', query: { ...route.query, cursor: undefined, ...values } });
}
async function selectedScope() {
  const targets = selectedItems.value.flatMap(i => i.target ? [i.target] : []);
  await change({ scope: targets.map(t => t.assetId), scopeInc: targets.map(t => t.incarnation), asset: undefined, incarnation: undefined, trail: undefined });
}
async function filters(predicates: model.Predicate[]) {
  const json = model.Query.toJson(model.Query.create({ predicates }));
  if (!json || typeof json !== 'object' || Array.isArray(json)) throw Error('Invalid query definition');
  editor.value = ''; await change({ filter: predicates.length ? JSON.stringify(json.predicates) : undefined });
}
async function open(target: AssetRef | undefined) {
  if (!target) return;
  if (key(detail.value?.item?.target) === key(target)) { detailHeading.value?.focus({ preventScroll: true }); return; }
  loading.value = true;
  if (!detail.value) { returnQuery = { ...route.query }; listScroll = window.scrollY; listQuery = loadedQuery; listSelection = [...selected.value]; listFocus = document.activeElement instanceof HTMLElement ? document.activeElement : undefined; }
  const trail = parameters('trail');
  if (detail.value?.item?.target && key(detail.value.item.target) !== key(target)) trail.push(JSON.stringify({ ...route.query, trail: undefined }));
  await router.push({ path: '/', query: { ...route.query, origin: route.query.asset ? route.query.origin : JSON.stringify({...route.query,origin:undefined}), localSearch: undefined, localKind: undefined, localFilter: undefined, localCursor: undefined, trail: trail.length ? trail : undefined, asset: target.assetId, incarnation: target.incarnation } });
}
async function closeDetail() {
  const trail = parameters('trail'); const parent = trail.pop();
  if (parent) {
    try {
      const saved: unknown = JSON.parse(parent);
      if (!saved || typeof saved !== 'object' || !('asset' in saved) || !('incarnation' in saved) || typeof saved.asset !== 'string' || typeof saved.incarnation !== 'string') throw Error('The parent context is invalid');
      const state = saved as Record<string, unknown>;
      const restored: LocationQuery = {};
      for (const [field,value] of Object.entries(state)) {
        if (typeof value !== 'string' && value !== null && !(Array.isArray(value) && value.every(v => v === null || typeof v === 'string'))) throw Error('The parent selection is invalid');
        restored[field] = value;
      }
      await router.push({ path: '/', query: { ...restored, trail: trail.length ? trail : undefined } });
    } catch (e) { error.value = String(e); }
  } else if (returnQuery) {const query=returnQuery; returnQuery=undefined; await router.push({path:'/',query});}
  else if (typeof route.query.origin==='string') {
    try {
      const restored:unknown=JSON.parse(route.query.origin);
      if (!restored || typeof restored!=='object' || Array.isArray(restored) || !Object.values(restored).every(v=>typeof v==='string'||v===null||(Array.isArray(v)&&v.every(x=>typeof x==='string'||x===null)))) throw Error('The enclosing selection is invalid');
      await router.push({path:'/',query:restored as LocationQuery});
    } catch(e){error.value=String(e);}
  }
  else await router.push({ path: '/', query: { ...route.query, asset: undefined, incarnation: undefined, trail: undefined, localSearch: undefined, localKind: undefined, localFilter: undefined, localCursor: undefined } });
}
watch(detail, async (value, previous) => {
  await nextTick();
  if (value && key(value.item?.target) !== key(previous?.item?.target)) {
    detailHeading.value?.focus({ preventScroll: true });
    if (window.matchMedia('(max-width: 700px)').matches) window.scrollTo(0, 0);
    else if (window.matchMedia('(max-width: 1349px)').matches) detailHeading.value?.scrollIntoView({ block: 'start' });
  }
  if (!value && previous && !error.value) {
    if (loadedQuery === listQuery) { selected.value = listSelection; if (listFocus?.isConnected) listFocus.focus({ preventScroll: true }); window.scrollTo(0, listScroll); }
    else window.scrollTo(0, 0);
  }
});
async function saved(target?: AssetRef) { editor.value = ''; notice.value = 'Changes saved to local storage.'; await metadata(); if (target) await open(target); else await load(); }
function create() { editing.value = undefined; editor.value = 'asset'; }
function edit() { editing.value = detail.value?.item; editor.value = 'asset'; }
async function useView(v: model.SavedView) {
  if (!v.query) return;
  const q = v.query;
  const json = model.Query.toJson(q);
  const filter = q.predicates.length && json && typeof json === 'object' && !Array.isArray(json) ? JSON.stringify(json.predicates) : undefined;
  await router.push({ path: '/', query: { view: String(q.view), search: q.focus ? undefined : q.search || undefined, localSearch: q.focus ? q.search || undefined : undefined, columns: q.columns.join(','), sort: q.sort, desc: q.descending ? '1' : undefined, unplaced: q.unplaced ? '1' : undefined, kind: q.focus ? undefined : q.kinds.map(String), localKind: q.focus ? q.kinds.map(String) : undefined, scope: q.roots.map(t => t.assetId), scopeInc: q.roots.map(t => t.incarnation), filter: q.focus ? undefined : filter, localFilter: q.focus ? filter : undefined, asset: q.focus?.assetId, incarnation: q.focus?.incarnation, saved: v.id } });
}
async function exportCsv() {
  try {
    const result = await call({ oneofKind: 'export', export: { query: query.value, spreadsheet: true } });
    if (result.oneofKind !== 'download') return;
    const url = URL.createObjectURL(new Blob([result.download.csv], { type: 'text/csv;charset=utf-8' }));
    const link = document.createElement('a'); link.href = url; link.download = 'omloom-inventory.csv'; link.click(); URL.revokeObjectURL(url);
  } catch (e) { error.value = String(e); }
}
function relations() { editor.value = 'relation'; }
function relatedName(target: AssetRef | undefined) {
  const related = detail.value?.related.find(i => key(i.target) === key(target));
  return display(related?.fields.find(f => f.name === 'name')) || target?.assetId.slice(-8);
}
async function removeParticipant(participant: model.Participant) {
  try {
    await call({ oneofKind: 'member', member: model.Member.create({ address: detail.value?.item?.target, expected: detail.value?.item?.revision, expectedGroup: detail.value?.group?.revision, interface: participant.interface, remove: true }) }, enclosing.value); await saved();
  } catch (e) { error.value = String(e); }
}
async function removeRelation(relation: import('./api').Relation) {
  try { await call({ oneofKind: 'relation', relation: { relation, remove: true } }, working.value); await saved(); } catch (e) { error.value = String(e); }
}
function allocate() { editing.value = undefined; editor.value = 'network'; }
watch(() => route.fullPath, load);
onMounted(async () => { try { await metadata(); await load(); } catch (e) { error.value = String(e); } });
</script>
<template><div class="shell" :class="{ 'has-detail': !!detail?.item }"><aside class="sidebar"><a class="brand" href="#/">omloom<span>Infrastructure workspace</span></a><p class="nav-caption">WORKSPACE</p><nav aria-label="Views"><button v-for="v in presentations" :key="v.id" :class="{ active: query.view === v.id }" @click="change({ view: String(v.id), kind: undefined })"><span aria-hidden="true">{{ v.symbol }}</span>{{ v.name }}</button></nav>
<div class="saved-heading"><p class="nav-caption">SAVED VIEWS</p><button aria-label="Save current view" @click="editor = 'view'">+</button></div><input v-if="views.length" v-model="viewSearch" aria-label="Find saved views" placeholder="Find a saved view" maxlength="100"><button v-for="v in matchingViews" :key="v.id" class="saved-link" @click="useView(v)">{{ v.audiences.length ? '♧' : '◦' }} {{ v.name }}</button><p v-if="!views.length" class="sidebar-hint">Keep useful selections here.</p><p v-else-if="!matchingViews.length" class="sidebar-hint">No visible definitions match.</p>
<div class="fixture"><span class="status-dot" />Local development<strong>{{ identity?.name ?? 'Connecting…' }}</strong><small>Fixture scope: {{ identity?.fixture }}</small></div></aside>
<main><header class="topbar"><div class="breadcrumb">Workspace <span>/</span> {{ currentTitle }}</div><button @click="load" :disabled="loading">↻ Refresh</button></header>
<div class="workspace-head"><div><h1>{{ currentTitle }}</h1></div><div class="actions workspace-actions" @keydown.esc="moreActions = false"><div id="workspace-secondary" class="secondary-actions" :class="{ open: moreActions }"><button class="phone-refresh" @click="load" :disabled="loading">Refresh inventory</button><button @click="exportCsv">Export CSV</button><button v-if="identity?.writable" @click="editor = 'import'">Import CSV</button><button v-if="identity?.schemaAdmin" @click="editor = 'schema'">Company fields</button></div><button v-if="identity?.writable" class="primary" @click="create">+ Create resource</button><button class="phone-more" :aria-expanded="moreActions" aria-controls="workspace-secondary" @click="moreActions = !moreActions">More</button></div></div>
<div class="scopebar"><span class="scope-label">SCOPE</span><template v-if="detail?.item"><strong>{{ detailName || detail.item.target?.assetId.slice(-8) }}</strong><button @click="closeDetail">Enclosing selection</button></template><button v-else-if="query.roots.length" @click="change({ scope: undefined, scopeInc: undefined })">{{ query.roots.length }} selected root{{ query.roots.length === 1 ? '' : 's' }} <span aria-hidden="true">×</span></button><span v-else>All permitted infrastructure</span><small>{{ detail?.item ? 'Local components and known facts' : 'Changing views preserves this scope.' }}</small></div>
<CategoryNav :view="query.view" :kinds="query.kinds" @select="kinds => change({kind:kinds.length?kinds.map(String):undefined})" />
<p v-if="notice" class="notice" role="status">{{ notice }}<button aria-label="Dismiss notice" @click="notice = ''">×</button></p><p v-if="error" class="error" role="alert">{{ error }}<button @click="change({ cursor: undefined })">Reload selection</button></p>
<div class="workspace-content" :class="{ split: !!detail?.item }"><section class="inventory" aria-label="Inventory"><div class="inventory-toolbar"><form class="search" @submit.prevent="change({ search: search || undefined })"><input v-model="search" aria-label="Search inventory" placeholder="Search inventory…" maxlength="256"><button>Search</button></form><div class="query-tools"><button aria-label="Resource kinds" @click="editor = 'kinds'">Resource kinds<span v-if="query.kinds.length"> ({{ query.kinds.length }})</span></button><button @click="editor = 'filter'">Add filter<span v-if="query.predicates.length"> ({{ query.predicates.length }})</span></button><label class="check"><input :checked="query.unplaced" type="checkbox" @change="change({ unplaced: ($event.target as HTMLInputElement).checked ? '1' : undefined })">Unplaced</label></div></div>
<div class="table-meta"><span>{{ page.total.toString() }} resources<span v-if="loading"> · Loading…</span></span><div><button @click="editor = 'columns'">Columns</button><button @click="editor = 'view'">Save view</button><button v-if="activeView" @click="useView(activeView)">Reset adjustments</button><button v-if="identity?.writable && query.view === model.View.NETWORK && actions.allocations.length" @click="allocate">Allocate IP / prefix</button></div></div>
<div v-if="query.predicates.length" class="selection"><span v-for="(p, index) in query.predicates" :key="index" class="chip">{{ definitions.find(d => d.id === p.field)?.name ?? p.field }} = {{ display(p.value) }}<button :aria-label="`Remove filter ${index + 1}`" @click="filters(query.predicates.filter((_, i) => i !== index))">×</button></span></div>
<div v-if="selected.length" class="selection">{{ selected.length }} selected on this page<button @click="selectedScope">Use selected as scope</button><button v-if="identity?.writable" @click="editor = 'bulk'">Review bulk change</button><button @click="selected = []">Clear selection</button></div>
<div v-if="page.items.length" class="phone-list-tools"><label class="check"><input type="checkbox" aria-label="Select phone page" :checked="selected.length === page.items.length" @change="selected = ($event.target as HTMLInputElement).checked ? page.items.map(i => key(i.target)) : []">Select page</label><label>Sort<select aria-label="Sort resources" :value="`${query.sort}:${query.descending ? 'desc' : 'asc'}`" @change="change({ sort: ($event.target as HTMLSelectElement).value.split(':')[0], desc: ($event.target as HTMLSelectElement).value.endsWith(':desc') ? '1' : undefined })"><option value="name:asc">Name A–Z</option><option value="name:desc">Name Z–A</option><option value="kind:asc">Resource kind</option></select></label></div>
<div class="table-scroll"><table :class="{ 'has-name': query.columns.includes('name') }"><thead><tr><th class="select-col"><input aria-label="Select visible page" type="checkbox" :checked="!!page.items.length && selected.length === page.items.length" @change="selected = ($event.target as HTMLInputElement).checked ? page.items.map(i => key(i.target)) : []"></th><th><button @click="change({ sort: 'kind', desc: query.sort === 'kind' && !query.descending ? '1' : undefined })">Resource kind <span v-if="query.sort === 'kind'">{{ query.descending ? '↓' : '↑' }}</span></button></th><th v-for="c in columnLabels" :key="c.id"><button v-if="c.id === 'name'" @click="change({ sort: 'name', desc: query.sort === 'name' && !query.descending ? '1' : undefined })">{{ c.name }} <span v-if="query.sort === 'name'">{{ query.descending ? '↓' : '↑' }}</span></button><span v-else>{{ c.name }}</span></th><th>Identity</th></tr></thead><tbody><template v-for="(group, groupIndex) in rowGroups" :key="groupIndex"><tr v-if="grouping && group.title" class="context-group"><td :colspan="columnLabels.length + 3"><button v-if="group.owner" :aria-label="`Open owner ${group.title}`" @click="open(group.owner)">{{ group.title }}</button><strong v-else>{{ group.title }}</strong><small>Resources on this page</small></td></tr><tr v-for="i in group.items" :key="key(i.target)" :class="{ focused: key(i.target) === key(detail?.item?.target) }"><td><input v-model="selected" type="checkbox" :value="key(i.target)" :aria-label="`Select ${display(i.fields.find(f => f.name === 'name'))}`"></td><td><span class="kind-tag">{{ label(i.kind) }}</span></td><td v-for="c in columnLabels" :key="c.id" :data-label="c.name" :class="{ 'empty-field': c.id !== 'name' && !display(i.fields.find(f => f.name === c.id)) }"><div v-if="c.id === 'name' && rowAddresses(i)" class="resource-address">{{ rowAddresses(i) }}</div><small v-if="c.id === 'name' && [K.SERVICE, K.INTERFACE].includes(i.kind)" class="resource-owner">{{ contextPath(i) || 'Owner not known' }}</small><button v-if="c.id === 'name'" class="resource-link" @click="open(i.target)">{{ display(i.fields.find(f => f.name === c.id)) || 'Unnamed resource' }}</button><span v-else>{{ display(i.fields.find(f => f.name === c.id)) || '·' }}</span></td><td><button class="identity-link" @click="open(i.target)">{{ i.target?.assetId.slice(-8) }} ↗</button></td></tr></template></tbody></table></div>
<div v-if="!page.items.length && !loading && !error" class="empty"><span aria-hidden="true">▦</span><h2>{{ initialEmpty ? 'Start with the infrastructure you know' : query.levelOnly ? 'No resources at this level in this view' : 'No resources match this selection' }}</h2><p>{{ initialEmpty ? 'Create a resource or preview a CSV import. Each object keeps its own stable identity.' : query.levelOnly ? 'Choose another view or open an enclosing resource to inspect its contents. Nested resources are shown inside their owners.' : 'Try another term or change the scope. Search only includes fields you may read.' }}</p><button v-if="identity?.writable" class="primary" @click="create">Create resource</button></div>
<footer class="pagination"><span>Current accepted state · revision {{ page.revision.toString() }}</span><div><button v-if="query.cursor" @click="change({ cursor: undefined })">First page</button><button :disabled="!page.next" @click="change({ cursor: page.next })">Next page →</button></div></footer></section>
<section v-if="detail?.item" class="detail" aria-label="Resource details"><button class="back-to-list" @click="closeDetail">{{ parameters('trail').length ? 'Back to parent' : 'Back to inventory' }}</button><header><div><p class="eyebrow">{{ label(detail.item.kind) }}</p><h2 ref="detailHeading" tabindex="-1">{{ detailName || 'Resource details' }}</h2></div><div class="actions"><button @click="change({ scope: detail.item.target?.assetId, scopeInc: detail.item.target?.incarnation, asset: undefined, incarnation: undefined, trail: undefined })">Use as scope</button><button v-if="identity?.writable" @click="edit">Edit resource</button><button aria-label="Close resource details" @click="closeDetail">×</button></div></header><dl><template v-for="f in detail.item.fields" :key="f.name"><dt>{{ definitions.find(d => d.id === f.name)?.name ?? f.name.replaceAll('_',' ') }}</dt><dd>{{ display(f) }}</dd></template><dt>Stable identity</dt><dd class="mono">{{ detail.item.target?.assetId }}</dd><dt>Incarnation</dt><dd class="mono">{{ detail.item.target?.incarnation }}</dd><dt>Accepted revision</dt><dd>{{ detail.item.revision.toString() }}</dd></dl>
<section class="phone-context" aria-label="Local resources"><label for="local-perspective">Resource perspective</label><select id="local-perspective" :value="query.view" @change="change({ view: ($event.target as HTMLSelectElement).value, kind: undefined })"><option v-for="v in presentations" :key="v.id" :value="v.id">{{ v.name }}</option></select><div class="actions"><button v-if="identity?.writable && actions.creations.length" class="primary" @click="create">+ Create resource</button><button v-if="identity?.writable && query.view === model.View.NETWORK && actions.allocations.length" @click="allocate">Allocate IP / prefix</button></div><ul><li v-for="item in page.items.filter(i => key(i.target) !== key(detail?.item?.target))" :key="key(item.target)"><button @click="open(item.target)">{{ display(item.fields.find(f => f.name === 'name')) || item.target?.assetId.slice(-8) }}</button><span>{{ label(item.kind) }}</span></li></ul><button v-if="page.next" @click="change({ cursor: page.next })">Next local resources</button></section>
<section v-if="[K.IP_ADDRESS, K.PREFIX, K.VLAN, K.ADDRESS_GROUP].includes(detail.item.kind)"><div class="section-title"><h3>Address allocation</h3><button v-if="identity?.writable" @click="editing = detail.item; editor = 'network'">Review network settings</button><button v-if="identity?.writable && [K.IP_ADDRESS, K.ADDRESS_GROUP].includes(detail.item.kind)" @click="editor = 'member'">Assign participant</button></div><p v-if="detail.group">Shared group: <button @click="open(detail.group.target)">{{ display(detail.group.fields.find(f => f.name === 'name')) }} · {{ detail.group.target?.assetId.slice(-8) }}</button></p><p v-if="!detail.participantsComplete" class="hint">Participant coverage is restricted. Missing participants do not prove the allocation is free.</p><ul><li v-for="p in detail.participants" :key="key(p.interface)"><button @click="open(p.interface)">Interface {{ p.interface?.assetId.slice(-8) }}</button><span v-if="p.priority !== undefined"> · Intended priority {{ p.priority }}</span><button v-if="identity?.writable" @click="removeParticipant(p)">Remove participant</button></li></ul></section>
<button v-if="detail.item.kind === K.ADDRESS_REFERENCE && identity?.writable" @click="editor = 'binding'">Resolve address in broader scope</button>
<CurrentAddresses v-for="value in detail.currentAddresses" :key="value.coverage?.source" :value="value" />
<HostNetwork v-for="evidence in detail.hostNetwork" :key="key(detail.item.target)+'/'+evidence.coverage?.source" :evidence="evidence" @open="open" />
<div class="section-title"><h3>Typed relationships</h3><button v-if="identity?.writable" @click="relations">Connections in broader scope</button></div><p v-if="!detail.relations.length" class="hint">No relationships visible in this scope. This does not establish absence outside it.</p><ul><li v-for="r in detail.relations" :key="r.relationId"><button @click="open(key(r.from) === key(detail.item?.target) ? r.to : r.from)">{{ RelationKind[r.kind]?.replace('RELATION_KIND_', '').toLowerCase().replaceAll('_',' ') }} · {{ relatedName(key(r.from) === key(detail.item?.target) ? r.to : r.from) }}</button><button v-if="identity?.writable" @click="removeRelation(r)">Remove relationship</button></li></ul>
<button v-if="detail.relationsNext" @click="nextRelations">Next relationships</button>
<template v-for="section in [{ title: 'Observed evidence', items: detail.observed }, { title: 'Import proposals', items: detail.imported }]" :key="section.title"><h3>{{ section.title }}</h3><p v-if="!section.items.length" class="hint">{{ detail.hostNetwork.length && section.title === 'Observed evidence' ? 'No additional visible field evidence.' : 'No visible current evidence. Accepted values are not observations.' }}</p><div v-for="o in section.items" :key="o.evidence?.sourceId"><p class="mono">Source {{ o.evidence?.sourceId }}</p><dl><template v-for="f in o.evidence?.fields" :key="f.name"><dt>{{ f.name }}</dt><dd>{{ display(f) }}</dd></template></dl></div></template></section>
</div></main></div>
<AssetEditor v-if="editor === 'asset'" :item="editing" :initial-kind="initialKind" :initial-hypervisor="query.view === model.View.VIRTUALIZATION && query.kinds.length === 1 && query.kinds[0] === K.DEVICE" :definitions="definitions" :working="working" :view="query.view" @close="editor = ''" @saved="saved" />
<SchemaEditor v-if="editor === 'schema'" :definitions="definitions" :revision="page.revision" @close="editor = ''" @saved="saved" />
<ImportEditor v-if="editor === 'import'" :definitions="definitions" :working="working" @close="editor = ''" @saved="saved" />
<ViewEditor v-if="editor === 'view' && identity" :query="query" :identity="identity" :existing="activeView" @close="editor = ''" @saved="saved" />
<NetworkEditor v-if="editor === 'network'" :item="editing" :group="editing ? detail?.group : undefined" :definitions="definitions" :working="editing ? enclosing : working" :allocations="actions.allocations" :view="query.view" @close="editor = ''" @saved="saved" @refresh="load" />
<MemberEditor v-if="editor === 'member' && detail" :detail="detail" :working="enclosing" @close="editor = ''" @saved="saved" />
<FilterEditor v-if="editor === 'filter'" :definitions="definitions" :predicates="query.predicates" @close="editor = ''" @apply="filters" />
<BulkEditor v-if="editor === 'bulk'" :items="selectedItems" :revision="page.revision" :definitions="definitions" :working="working" @close="editor = ''" @saved="saved" />
<RelationEditor v-if="editor === 'relation' && detail?.item?.target" :target="detail.item.target" :working="enclosing" @close="editor = ''" @saved="saved" />
<AddressBinding v-if="editor === 'binding' && detail?.item" :item="detail.item" :working="enclosing" :definitions="definitions" @close="editor = ''" @saved="saved" />
<SelectionEditor v-if="editor === 'kinds'" title="Resource kinds" :options="kindOptions" :selected="query.kinds.map(String)" :allow-empty="true" @close="editor = ''" @apply="values => { editor = ''; change({ kind: values }); }" />
<SelectionEditor v-if="editor === 'columns'" title="Columns" :options="columnOptions" :selected="query.columns" :allow-empty="false" @close="editor = ''" @apply="values => { editor = ''; change({ columns: values.join(',') }); }" />
</template>
