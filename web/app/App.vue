<script setup lang="ts">
import HostNetwork from './HostNetwork.vue';
import HostImportEditor from './HostImportEditor.vue';
import CategoryNav from './CategoryNav.vue';
import CurrentAddresses from './CurrentAddresses.vue';
import HostIdentityEvidence from './HostIdentityEvidence.vue';
import HostIdentityCompare from './HostIdentityCompare.vue';
import VmPlacement from './VmPlacement.vue';
import ResponsibilityPanel from './ResponsibilityPanel.vue';
import AccountPanel from './AccountPanel.vue';
import { softwareFields } from './software-fields';
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
import { call, model, label, display, key, kinds, enumLabel, AssetKind as K, RelationKind, text, type AssetRef } from './api';
const router = useRouter(); const route = useRoute();
const identity = ref<model.Identity>(); const definitions = ref<model.Definition[]>([]); const views = ref<model.SavedView[]>([]);
const page = ref(model.Page.create()); const detail = ref<model.Detail>(); const loading = ref(false); const error = ref(''); const notice = ref('');
const networkCursor = ref<string>(); const networkLoading = ref(false);
const preparedActions = ref(model.Actions.create());
const participantRemoval = ref<model.Participant>();
const actions = computed(() => loading.value ? model.Actions.create() : preparedActions.value);
const editor = ref<'' | 'asset' | 'schema' | 'import' | 'host-import' | 'view' | 'network' | 'bulk' | 'relation' | 'member' | 'filter' | 'kinds' | 'columns' | 'binding'>(''); const editing = ref<model.Item>();
const initialKind = computed(() => query.value.kinds.length===1 ? query.value.kinds[0] as K : ({ [model.View.OVERVIEW]: K.DEVICE, [model.View.NETWORK]: K.ADDRESS_REFERENCE, [model.View.HARDWARE]: K.DEVICE, [model.View.VIRTUALIZATION]: K.VIRTUAL_MACHINE, [model.View.SERVICES]: K.SERVICE })[query.value.view]);
const kindOptions = kinds.map(k => ({ id: String(k), name: label(k) }));
const columnOptions = computed(() => {
  const options = [{ id: 'name', name: 'Name' }, { id: 'dns_name', name: 'DNS name' }, { id: 'description', name: 'Description' }, { id: 'serial', name: 'Serial number' }, ...softwareFields(), ...definitions.value.filter(d => !d.retired)];
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
const identityPair = ref<{left:AssetRef;right:AssetRef;working:model.WorkingContext}>();
const placementVm = ref<AssetRef>();
async function placementSaved(vm: AssetRef, host: AssetRef) {
  placementVm.value = undefined;
  await router.push({path:'/',query:{view:String(model.View.VIRTUALIZATION),scope:[host.assetId],scopeInc:[host.incarnation],asset:vm.assetId,incarnation:vm.incarnation}});
}
const identityCandidates=ref<model.Item[]>();const findingCandidates=ref(false);
const resolvedAlias=ref<model.ResolvedIdentity>();
const comparable = computed(() => {
  const items = page.value.items.filter(item => selected.value.includes(key(item.target)));
  return items.length === 2 && items.every(item => item.target && [K.DEVICE, K.VIRTUAL_MACHINE, K.SOFTWARE_INSTALLATION, K.INTERFACE].includes(item.kind));
});
function compareIdentity() {
  const items = page.value.items.filter(item => selected.value.includes(key(item.target)));
  if (comparable.value && items[0]?.target && items[1]?.target) identityPair.value = {left:items[0].target,right:items[1].target,working:model.WorkingContext.create(working.value)};
}
async function findIdentityCandidates(){
  const current=detail.value;if(!current?.item?.target||findingCandidates.value)return;
  findingCandidates.value=true;error.value='';identityCandidates.value=undefined;
  try{const value=await call({oneofKind:'findIdentityCandidates',findIdentityCandidates:model.FindIdentityCandidates.create({target:current.item.target})},enclosing.value);if(value.oneofKind!=='identityCandidates')throw Error('Candidate search is unavailable.');if(detail.value===current)identityCandidates.value=value.identityCandidates.items;}
  catch(e){if(detail.value===current)error.value=String(e);}finally{findingCandidates.value=false;}
}
function compareCandidate(item:model.Item){const left=detail.value?.item?.target;if(left&&item.target)identityPair.value={left,right:item.target,working:model.WorkingContext.create(enclosing.value)};}
async function resolveIdentityReference(){
  if(typeof route.query.asset!=='string'||typeof route.query.incarnation!=='string')return;
  try{const value=await call({oneofKind:'resolveIdentity',resolveIdentity:model.ResolveIdentity.create({original:{assetId:route.query.asset,incarnation:route.query.incarnation}})},enclosing.value);if(value.oneofKind!=='resolvedIdentity')throw Error('Reference resolution is unavailable.');resolvedAlias.value=value.resolvedIdentity;}
  catch(e){error.value=String(e);}
}
const viewSearch = ref('');
const matchingViews = computed(() => views.value.filter(v => v.name.toLocaleLowerCase().includes(viewSearch.value.toLocaleLowerCase())));
const presentations = [{ id: model.View.OVERVIEW, name: 'Overview', symbol: '▦' }, { id: model.View.NETWORK, name: 'Network', symbol: '⇄' }, { id: model.View.HARDWARE, name: 'Hardware', symbol: '▤' }, { id: model.View.VIRTUALIZATION, name: 'Virtualization', symbol: '◫' }, { id: model.View.SERVICES, name: 'Software & services', symbol: '◇' }];
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
    && !route.query[route.query.asset ? 'localFilter' : 'filter'] && !route.query.responsibility && route.query.unplaced !== '1',
  responsibility: parseResponsibility(),
}));
const working = computed(() => model.WorkingContext.create({ roots: query.value.roots, focus: query.value.focus, expected: page.value.revision }));
const enclosing = computed(() => model.WorkingContext.create({ roots: query.value.roots, expected: page.value.revision }));
function parsePredicates(parameter: string): model.Predicate[] {
  if (!route.query[parameter]) return [];
  try { const json = JSON.parse(String(route.query[parameter])); return model.Query.fromJson({ predicates: json }).predicates; } catch { return [{ field: 'unavailable', value: text('unavailable', '') }]; }
}
function parseResponsibility(): model.ResponsibilityFilter|undefined {
  if(!route.query.responsibility)return undefined;
  try{return model.ResponsibilityFilter.fromJson(JSON.parse(String(route.query.responsibility)));}
  catch{return model.ResponsibilityFilter.create();}
}
const activeView = computed(() => views.value.find(v => v.id === route.query.saved));
const currentTitle = computed(() => presentations.find(v => v.id === query.value.view)?.name ?? 'Inventory');
const initialEmpty = computed(()=>query.value.view===model.View.OVERVIEW && !query.value.focus && !query.value.roots.length && !query.value.search && !query.value.kinds.length && !query.value.predicates.length && !query.value.responsibility && !query.value.unplaced);
const columnLabels = computed(() => query.value.columns.map(id => ({ id, name: definitions.value.find(d => d.id === id)?.name ?? id.replaceAll('_', ' ') })));
const selectedItems = computed(() => page.value.items.filter(i => selected.value.includes(key(i.target))));
const rowContexts = computed(() => new Map(page.value.contexts.map(c => [key(c.target), c])));
const responsibilityGrouping = computed(() => query.value.kinds.length === 1 && query.value.kinds[0] === K.WORKLOAD);
const grouping = computed(() => responsibilityGrouping.value || !query.value.focus);
function workloadOwner(item: model.Item) { return rowContexts.value.get(key(item.target))?.responsibility; }
function ownerCaption(item: model.Item): string {
  const context = rowContexts.value.get(key(item.target));
  if (!context) return 'Owner not loaded';
  const owner = context.responsibility;
  if (!owner) return 'Ownership restricted';
  if (owner.party) return `${enumLabel(model.PartyKind, owner.party.kind)}: ${owner.party.name}`;
  if (owner.status === model.ResponsibilityStatus.UNKNOWN) return 'Owner not established';
  return `Owner: ${enumLabel(model.ResponsibilityStatus, owner.status).replaceAll('_', ' ')}`;
}
async function filterOwner(item: model.Item) {
  const party = workloadOwner(item)?.party;
  if (!party) return;
  await filters(query.value.predicates, model.ResponsibilityFilter.create({ facet: model.ResponsibilityFacet.CLAIMED_OWNER, issuer: party.issuer, subject: party.subject }));
}
function contextPath(item: model.Item) {
  const context=rowContexts.value.get(key(item.target));
  if (context?.alternativeOwners.length) return `Shared: ${context.alternativeOwners.map(o=>o.name || label(o.kind)).join(' · ')}`;
  return context?.owners.map(o => o.name || label(o.kind)).join(' / ') || '';
}
const rowGroups = computed(() => {
  const groups = new Map<string, { title: string; owner?: AssetRef; items: model.Item[] }>();
  for (const item of page.value.items) {
    const context = rowContexts.value.get(key(item.target));
    const byResponsibility = responsibilityGrouping.value && item.kind === K.WORKLOAD;
    const title = byResponsibility ? ownerCaption(item) : grouping.value ? contextPath(item) : '';
    const owner = byResponsibility ? undefined : context?.owners.at(-1)?.target;
    const party = context?.responsibility?.party;
    const groupKey = byResponsibility ? party ? `party:${JSON.stringify([party.issuer, party.subject])}` : ownerCaption(item)
      : grouping.value ? [...context?.owners ?? [],...context?.alternativeOwners ?? []].map(o => key(o.target)).join('/') || 'roots' : 'local';
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
  identityCandidates.value=undefined;resolvedAlias.value=undefined;
  const turn = ++generation; loading.value = true; error.value = '';
  preparedActions.value = model.Actions.create();
  const signature = JSON.stringify(model.Query.toJson(query.value));
  if (signature !== loadedQuery) { selected.value = []; search.value = query.value.search; loadedQuery = signature; }
  const target = route.query.asset && route.query.incarnation ? { assetId: String(route.query.asset), incarnation: String(route.query.incarnation) } : undefined;
  const detailLoad = target ? read(target).then(next => {
    if (turn === generation) { detail.value = next; networkCursor.value = undefined; }
  }).catch(e => { if (turn === generation) { detail.value = undefined; error.value = String(e); } }) : Promise.resolve();
  try {
    const result = await call({ oneofKind: 'query', query: query.value });
    if (turn !== generation) return;
    if (result.oneofKind !== 'page') throw Error('Unexpected inventory response');
    page.value = result.page;
    if (!target) { detail.value = undefined; networkCursor.value = undefined; }
    const prepared = await call({ oneofKind: 'prepare', prepare: model.Prepare.create({ view: query.value.view }) }, working.value).catch(e=>{ if(turn===generation) error.value=String(e); return undefined; });
    if (turn !== generation) return;
    if (prepared?.oneofKind === 'actions') preparedActions.value = prepared.actions;
    selected.value = selected.value.filter(id => result.page.items.some(item => key(item.target) === id));
  } catch (e) { if (turn === generation) { page.value = model.Page.create(); if (!target) detail.value = undefined; error.value = String(e); } } finally { await detailLoad; if (turn === generation) loading.value = false; }
}
async function metadata() {
  const [i, d, v] = await Promise.all([call({ oneofKind: 'identity', identity: {} }), call({ oneofKind: 'definitions', definitions: {} }), call({ oneofKind: 'views', views: {} })]);
  if (i.oneofKind === 'identity') identity.value = i.identity; if (d.oneofKind === 'definitions') definitions.value = d.definitions.items; if (v.oneofKind === 'views') views.value = v.views.items;
}
async function read(target: AssetRef, relationsCursor?: string, networkCursor?: string) {
  const result = await call({ oneofKind: 'read', read: { target, relationsCursor, networkCursor } }, model.WorkingContext.create({ roots: query.value.roots, focus: target }));
  if (result.oneofKind !== 'detail') throw Error('Unexpected resource response');
  return result.detail;
}
async function networkEvidence(cursor?: string) {
  const current = detail.value; const target = current?.item?.target;
  if (!target || networkLoading.value) return;
  networkLoading.value = true;
  try {
    const next = await read(target, undefined, cursor);
    if (detail.value === current) {
      detail.value = cursor ? { ...current, hostNetwork: next.hostNetwork, networkNext: next.networkNext } : next;
      networkCursor.value = cursor;
    }
  } catch(e) { error.value = String(e); }
  finally { networkLoading.value = false; }
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
async function filters(predicates: model.Predicate[],responsibility?:model.ResponsibilityFilter) {
  const json = model.Query.toJson(model.Query.create({ predicates }));
  if (!json || typeof json !== 'object' || Array.isArray(json)) throw Error('Invalid query definition');
  editor.value = ''; await change({ filter: predicates.length ? JSON.stringify(json.predicates) : undefined, responsibility:responsibility ? JSON.stringify(model.ResponsibilityFilter.toJson(responsibility)) : undefined });
}
async function open(target: AssetRef | undefined, selection: LocationQuery = route.query) {
  if (!target) return;
  if (key(detail.value?.item?.target) === key(target)) { detailHeading.value?.focus({ preventScroll: true }); return; }
  loading.value = true;
  if (!detail.value) { returnQuery = { ...selection }; listScroll = window.scrollY; listQuery = loadedQuery; listSelection = [...selected.value]; listFocus = document.activeElement instanceof HTMLElement ? document.activeElement : undefined; }
  const trail = selection.trail === undefined ? [] : Array.isArray(selection.trail) ? selection.trail.map(String) : [String(selection.trail)];
  if (detail.value?.item?.target && key(detail.value.item.target) !== key(target)) trail.push(JSON.stringify({ ...selection, trail: undefined }));
  await router.push({ path: '/', query: { ...selection, origin: selection.asset ? selection.origin : JSON.stringify({...selection,origin:undefined}), localSearch: undefined, localKind: undefined, localFilter: undefined, localCursor: undefined, trail: trail.length ? trail : undefined, asset: target.assetId, incarnation: target.incarnation } });
}
async function openResolvedIdentity() {
  const original = resolvedAlias.value?.original;
  const canonical = resolvedAlias.value?.canonical?.target;
  if (!original || !canonical) return;
  const scope = roots().map(root => key(root) === key(original) ? canonical : root);
  await open(canonical, { ...route.query, scope: scope.map(root => root.assetId), scopeInc: scope.map(root => root.incarnation) });
}
async function closeDetail() {
  const trail = parameters('trail'); const parent = trail.pop();
  if (parent) {
    try {
      const restored = selectionState(parent);
      if (typeof restored.asset !== 'string' || typeof restored.incarnation !== 'string') throw Error('The parent context is invalid');
      await router.push({ path: '/', query: { ...restored, trail: trail.length ? trail : undefined } });
    } catch (e) { error.value = String(e); }
  } else if (returnQuery) {const query=returnQuery; returnQuery=undefined; await router.push({path:'/',query});}
  else if (typeof route.query.origin==='string') {
    try {
      await router.push({path:'/',query:selectionState(route.query.origin)});
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
    if (loadedQuery === listQuery) { selected.value = listSelection; await nextTick(); if (listFocus?.isConnected) listFocus.focus({ preventScroll: true }); window.scrollTo(0, listScroll); }
    else window.scrollTo(0, 0);
  }
});
function selectionState(encoded: string): LocationQuery {
  const value: unknown = JSON.parse(encoded);
  if (!value || typeof value !== 'object' || Array.isArray(value) || !Object.values(value).every(v => typeof v === 'string' || v === null || (Array.isArray(v) && v.every(x => typeof x === 'string' || x === null)))) throw Error('The saved selection is invalid');
  return value as LocationQuery;
}
function withoutContinuation(selection: LocationQuery, origin = true): LocationQuery {
  const next = { ...selection };
  delete next.cursor; delete next.localCursor;
  if (origin && typeof next.origin === 'string') next.origin = JSON.stringify(withoutContinuation(selectionState(next.origin), false));
  return next;
}
async function saved(target?: AssetRef) {
  loading.value = true;
  editor.value = ''; notice.value = 'Changes saved to local storage.';
  try {
    const next = withoutContinuation(route.query);
    const trail = parameters('trail');
    if (trail.length) next.trail = trail.map(value => JSON.stringify(withoutContinuation(selectionState(value))));
    if (returnQuery) returnQuery = withoutContinuation(returnQuery);
    await metadata();
    if (target && key(target) !== key(detail.value?.item?.target)) await open(target, next);
    else {
      const changed = router.resolve({ path: '/', query: next }).fullPath !== route.fullPath;
      await router.replace({ path: '/', query: next });
      if (!changed) await load();
    }
  } catch (e) { error.value = String(e); loading.value = false; }
}
async function reloadSelection() {
  const cursor = route.query.asset ? 'localCursor' : 'cursor';
  if (route.query[cursor] !== undefined) await change({ cursor: undefined });
  else await load();
}
function create() { if (loading.value) return; editing.value = undefined; editor.value = 'asset'; }
function edit() { editing.value = detail.value?.item; editor.value = 'asset'; }
async function useView(v: model.SavedView) {
  if (!v.query) return;
  const q = v.query;
  const json = model.Query.toJson(q);
  const filter = q.predicates.length && json && typeof json === 'object' && !Array.isArray(json) ? JSON.stringify(json.predicates) : undefined;
  await router.push({ path: '/', query: { view: String(q.view), search: q.focus ? undefined : q.search || undefined, localSearch: q.focus ? q.search || undefined : undefined, columns: q.columns.join(','), sort: q.sort, desc: q.descending ? '1' : undefined, unplaced: q.unplaced ? '1' : undefined, kind: q.focus ? undefined : q.kinds.map(String), localKind: q.focus ? q.kinds.map(String) : undefined, scope: q.roots.map(t => t.assetId), scopeInc: q.roots.map(t => t.incarnation), filter: q.focus ? undefined : filter, localFilter: q.focus ? filter : undefined, responsibility:q.responsibility?JSON.stringify(model.ResponsibilityFilter.toJson(q.responsibility)):undefined, asset: q.focus?.assetId, incarnation: q.focus?.incarnation, saved: v.id } });
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
function removeParticipant(participant: model.Participant) {
  participantRemoval.value = participant; editor.value = 'member';
}
async function removeRelation(relation: import('./api').Relation) {
  // Navigation can change the focus while this relationship is still visible.
  const roots = [relation.from, relation.to].filter((target): target is AssetRef => !!target);
  try { await call({ oneofKind: 'relation', relation: { relation, remove: true } }, model.WorkingContext.create({ roots, expected: page.value.revision })); await saved(); } catch (e) { error.value = String(e); }
}
function allocate() { editing.value = undefined; editor.value = 'network'; }
watch(() => route.fullPath, load);
onMounted(async () => { try { await metadata(); await load(); } catch (e) { error.value = String(e); } });
</script>
<template><div class="shell" :class="{ 'has-detail': !!detail?.item }"><aside class="sidebar"><a class="brand" href="#/">omloom<span>Infrastructure workspace</span></a><p class="nav-caption">WORKSPACE</p><nav aria-label="Views"><button v-for="v in presentations" :key="v.id" :class="{ active: query.view === v.id }" @click="change({ view: String(v.id), kind: undefined })"><span aria-hidden="true">{{ v.symbol }}</span>{{ v.name }}</button></nav>
<div class="saved-heading"><p class="nav-caption">SAVED VIEWS</p><button aria-label="Save current view" @click="editor = 'view'">+</button></div><input v-if="views.length" v-model="viewSearch" aria-label="Find saved views" placeholder="Find a saved view" maxlength="100"><button v-for="v in matchingViews" :key="v.id" class="saved-link" @click="useView(v)">{{ v.audiences.length ? '♧' : '◦' }} {{ v.name }}</button><p v-if="!views.length" class="sidebar-hint">Keep useful selections here.</p><p v-else-if="!matchingViews.length" class="sidebar-hint">No visible definitions match.</p>
<div class="fixture"><span class="status-dot" />Local development<strong>{{ identity?.name ?? 'Connecting…' }}</strong><small>Fixture scope: {{ identity?.fixture }}</small></div></aside>
<main><header class="topbar"><div class="breadcrumb">Workspace <span>/</span> {{ currentTitle }}</div><button @click="load" :disabled="loading">↻ Refresh</button></header>
<div class="workspace-head"><div><h1>{{ currentTitle }}</h1></div><div class="actions workspace-actions" @keydown.esc="moreActions = false"><div id="workspace-secondary" class="secondary-actions" :class="{ open: moreActions }"><button class="phone-refresh" @click="load" :disabled="loading">Refresh inventory</button><button @click="exportCsv">Export CSV</button><button v-if="identity?.writable" @click="editor = 'import'">Import CSV</button><button v-if="identity?.writable" @click="moreActions = false; editor = 'host-import'">Import host report</button><button v-if="identity?.schemaAdmin" @click="editor = 'schema'">Company fields</button></div><button v-if="identity?.writable" class="primary" :disabled="loading" @click="create">+ Create resource</button><button class="phone-more" :aria-expanded="moreActions" aria-controls="workspace-secondary" @click="moreActions = !moreActions">More</button></div></div>
<div class="scopebar"><span class="scope-label">SCOPE</span><template v-if="detail?.item"><strong>{{ detailName || detail.item.target?.assetId.slice(-8) }}</strong><button @click="closeDetail">Enclosing selection</button></template><button v-else-if="query.roots.length" @click="change({ scope: undefined, scopeInc: undefined })">{{ query.roots.length }} selected root{{ query.roots.length === 1 ? '' : 's' }} <span aria-hidden="true">×</span></button><span v-else>All permitted infrastructure</span><small>{{ detail?.item ? 'Local components and known facts' : 'Changing views preserves this scope.' }}</small></div>
<CategoryNav :view="query.view" :kinds="query.kinds" @select="kinds => change({kind:kinds.length?kinds.map(String):undefined})" />
<p v-if="notice" class="notice" role="status">{{ notice }}<button aria-label="Dismiss notice" @click="notice = ''">×</button></p><p v-if="error" class="error" role="alert">{{ error }}<button @click="reloadSelection">Reload selection</button><button v-if="route.query.asset&&route.query.incarnation" @click="resolveIdentityReference">Locate retained identity</button></p>
<section v-if="resolvedAlias?.canonical" aria-label="Corrected reference"><p>Original reference {{ resolvedAlias.original?.assetId }}. Current permitted identity: {{ display(resolvedAlias.canonical.fields.find(v=>v.name==='name')) }}.</p><button @click="openResolvedIdentity">Open retained resource</button></section>
<div class="workspace-content" :class="{ split: !!detail?.item }"><section class="inventory" aria-label="Inventory"><div class="inventory-toolbar"><form class="search" @submit.prevent="change({ search: search || undefined })"><input v-model="search" aria-label="Search inventory" placeholder="Search inventory…" maxlength="256"><button>Search</button></form><div class="query-tools"><button aria-label="Resource kinds" @click="editor = 'kinds'">Resource kinds<span v-if="query.kinds.length"> ({{ query.kinds.length }})</span></button><button @click="editor = 'filter'">Add filter<span v-if="query.predicates.length"> ({{ query.predicates.length }})</span></button><label class="check"><input :checked="query.unplaced" type="checkbox" @change="change({ unplaced: ($event.target as HTMLInputElement).checked ? '1' : undefined })">Unplaced</label></div></div>
<div class="table-meta"><span>{{ page.total.toString() }} resources<span v-if="loading"> · Loading…</span></span><div><button @click="editor = 'columns'">Columns</button><button @click="editor = 'view'">Save view</button><button v-if="activeView" @click="useView(activeView)">Reset adjustments</button><button v-if="identity?.writable && query.view === model.View.NETWORK && actions.allocations.length" @click="allocate">Allocate IP / prefix</button></div></div>
<div v-if="query.predicates.length||query.responsibility" class="selection"><span v-for="(p, index) in query.predicates" :key="index" class="chip">{{ definitions.find(d => d.id === p.field)?.name ?? p.field }} = {{ display(p.value) }}<button :aria-label="`Remove filter ${index + 1}`" @click="filters(query.predicates.filter((_, i) => i !== index),query.responsibility)">×</button></span><span v-if="query.responsibility" class="chip">{{model.ResponsibilityFacet[query.responsibility.facet]?.toLowerCase().replaceAll('_',' ')}}: {{query.responsibility.issuer}} / {{query.responsibility.subject}}<button aria-label="Remove responsibility filter" @click="filters(query.predicates)">×</button></span></div>
<div v-if="selected.length" class="selection">{{ selected.length }} selected on this page<button :disabled="loading" @click="selectedScope">Use selected as scope</button><button v-if="comparable" :disabled="loading" @click="compareIdentity">Compare identity</button><button v-if="identity?.writable" :disabled="loading" @click="editor = 'bulk'">Review bulk change</button><button @click="selected = []">Clear selection</button></div>
<div v-if="page.items.length" class="phone-list-tools"><label class="check"><input type="checkbox" aria-label="Select phone page" :disabled="loading" :checked="selected.length === page.items.length" @change="selected = ($event.target as HTMLInputElement).checked ? page.items.map(i => key(i.target)) : []">Select page</label><label>Sort<select aria-label="Sort resources" :value="`${query.sort}:${query.descending ? 'desc' : 'asc'}`" @change="change({ sort: ($event.target as HTMLSelectElement).value.split(':')[0], desc: ($event.target as HTMLSelectElement).value.endsWith(':desc') ? '1' : undefined })"><option value="name:asc">Name A–Z</option><option value="name:desc">Name Z–A</option><option value="kind:asc">Resource kind</option></select></label></div>
<div class="table-scroll"><table :class="{ 'has-name': query.columns.includes('name') }"><thead><tr><th class="select-col"><input aria-label="Select visible page" type="checkbox" :disabled="loading" :checked="!!page.items.length && selected.length === page.items.length" @change="selected = ($event.target as HTMLInputElement).checked ? page.items.map(i => key(i.target)) : []"></th><th><button @click="change({ sort: 'kind', desc: query.sort === 'kind' && !query.descending ? '1' : undefined })">Resource kind <span v-if="query.sort === 'kind'">{{ query.descending ? '↓' : '↑' }}</span></button></th><th v-for="c in columnLabels" :key="c.id"><button v-if="c.id === 'name'" @click="change({ sort: 'name', desc: query.sort === 'name' && !query.descending ? '1' : undefined })">{{ c.name }} <span v-if="query.sort === 'name'">{{ query.descending ? '↓' : '↑' }}</span></button><span v-else>{{ c.name }}</span></th><th>Identity</th></tr></thead><tbody><template v-for="(group, groupIndex) in rowGroups" :key="groupIndex"><tr v-if="grouping && group.title" class="context-group"><td :colspan="columnLabels.length + 3"><button v-if="group.owner" :aria-label="`Open owner ${group.title}`" @click="open(group.owner)">{{ group.title }}</button><strong v-else>{{ group.title }}</strong><small>Resources on this page</small></td></tr><tr v-for="i in group.items" :key="key(i.target)" :class="{ focused: key(i.target) === key(detail?.item?.target) }"><td><input v-model="selected" type="checkbox" :disabled="loading" :value="key(i.target)" :aria-label="`Select ${display(i.fields.find(f => f.name === 'name'))}`"></td><td><span class="kind-tag">{{ label(i.kind) }}</span></td><td v-for="c in columnLabels" :key="c.id" :data-label="c.name" :class="{ 'empty-field': c.id !== 'name' && !display(i.fields.find(f => f.name === c.id)) }">
<div v-if="c.id === 'name' && rowAddresses(i)" class="resource-address">{{ rowAddresses(i) }}</div>
<small v-if="c.id === 'name' && [K.SERVICE, K.INTERFACE].includes(i.kind)" class="resource-owner">{{ contextPath(i) || 'Owner not known' }}</small>
<small v-if="c.id === 'name' && [K.WORKLOAD, K.CONTAINER].includes(i.kind)" class="resource-owner">{{ contextPath(i) || 'Execution context not available' }}</small>
<template v-if="c.id === 'name' && [K.WORKLOAD, K.CONTAINER].includes(i.kind)"><button v-if="workloadOwner(i)?.party" class="resource-owner responsibility-link" :aria-label="`Show resources owned by ${workloadOwner(i)?.party?.name}`" @click="filterOwner(i)">Claimed owner: {{ ownerCaption(i) }}</button><small v-else class="resource-owner">{{ ownerCaption(i) }}</small></template>
<button v-if="c.id === 'name'" class="resource-link" @click="open(i.target)">{{ display(i.fields.find(f => f.name === c.id)) || 'Unnamed resource' }}</button><span v-else>{{ display(i.fields.find(f => f.name === c.id)) || '·' }}</span>
</td><td><button class="identity-link" @click="open(i.target)">{{ i.target?.assetId.slice(-8) }} ↗</button></td></tr></template></tbody></table></div>
<div v-if="!page.items.length && !loading && !error" class="empty"><span aria-hidden="true">▦</span><h2>{{ initialEmpty ? 'Start with the infrastructure you know' : query.levelOnly ? 'No resources at this level in this view' : 'No resources match this selection' }}</h2><p>{{ initialEmpty ? 'Create a resource or preview a CSV import. Each object keeps its own stable identity.' : query.levelOnly ? 'Choose another view or open an enclosing resource to inspect its contents. Nested resources are shown inside their owners.' : 'Try another term or change the scope. Search only includes fields you may read.' }}</p><button v-if="identity?.writable" class="primary" @click="create">Create resource</button></div>
<footer class="pagination"><span>Current accepted state · revision {{ page.revision.toString() }}</span><div><button v-if="query.cursor" @click="change({ cursor: undefined })">First page</button><button :disabled="!page.next" @click="change({ cursor: page.next })">Next page →</button></div></footer></section>
<section v-if="detail?.item" class="detail" aria-label="Resource details"><button class="back-to-list" @click="closeDetail">{{ parameters('trail').length ? 'Back to parent' : 'Back to inventory' }}</button><header><div><p class="eyebrow">{{ label(detail.item.kind) }}</p><h2 ref="detailHeading" tabindex="-1">{{ detailName || 'Resource details' }}</h2></div><div class="actions"><button @click="change({ scope: detail.item.target?.assetId, scopeInc: detail.item.target?.incarnation, asset: undefined, incarnation: undefined, trail: undefined })">Use as scope</button><button v-if="identity?.writable" @click="edit">Edit resource</button><button aria-label="Close resource details" @click="closeDetail">×</button></div></header><dl><template v-for="f in detail.item.fields" :key="f.name"><dt>{{ definitions.find(d => d.id === f.name)?.name ?? f.name.replaceAll('_',' ') }}</dt><dd>{{ display(f) }}</dd></template><dt>Stable identity</dt><dd class="mono">{{ detail.item.target?.assetId }}</dd><dt>Incarnation</dt><dd class="mono">{{ detail.item.target?.incarnation }}</dd><dt>Accepted revision</dt><dd>{{ detail.item.revision.toString() }}</dd></dl>
<section class="phone-context" aria-label="Local resources"><label for="local-perspective">Resource perspective</label><select id="local-perspective" :value="query.view" @change="change({ view: ($event.target as HTMLSelectElement).value, kind: undefined })"><option v-for="v in presentations" :key="v.id" :value="v.id">{{ v.name }}</option></select><div class="actions"><button v-if="identity?.writable && actions.creations.length" class="primary" @click="create">+ Create resource</button><button v-if="identity?.writable && query.view === model.View.NETWORK && actions.allocations.length" @click="allocate">Allocate IP / prefix</button></div><ul><li v-for="item in page.items.filter(i => key(i.target) !== key(detail?.item?.target))" :key="key(item.target)"><button @click="open(item.target)">{{ display(item.fields.find(f => f.name === 'name')) || item.target?.assetId.slice(-8) }}</button><span>{{ label(item.kind) }}</span></li></ul><button v-if="page.next" @click="change({ cursor: page.next })">Next local resources</button></section>
<section v-if="[K.IP_ADDRESS, K.PREFIX, K.VLAN, K.ADDRESS_GROUP].includes(detail.item.kind)"><div class="section-title"><h3>Address allocation</h3><button v-if="identity?.writable" @click="editing = detail.item; editor = 'network'">Review network settings</button><button v-if="identity?.writable && [K.IP_ADDRESS, K.ADDRESS_GROUP].includes(detail.item.kind)" @click="editor = 'member'">Assign participant</button></div><p v-if="detail.group">Shared group: <button @click="open(detail.group.target)">{{ display(detail.group.fields.find(f => f.name === 'name')) }} · {{ detail.group.target?.assetId.slice(-8) }}</button></p><p v-if="!detail.participantsComplete" class="hint">Participant coverage is restricted. Missing participants do not prove the allocation is free.</p><ul><li v-for="p in detail.participants" :key="key(p.interface)"><button @click="open(p.interface)">Interface {{ p.interface?.assetId.slice(-8) }}</button><span v-if="p.priority !== undefined"> · Intended priority {{ p.priority }}</span><button v-if="identity?.writable" @click="removeParticipant(p)">Remove participant</button></li></ul></section>
<button v-if="detail.item.kind === K.ADDRESS_REFERENCE && identity?.writable" @click="editor = 'binding'">Resolve address in broader scope</button>
<CurrentAddresses v-for="value in detail.currentAddresses" :key="value.sourceId" :value="value" />
<section v-if="[K.DEVICE,K.VIRTUAL_MACHINE,K.SOFTWARE_INSTALLATION,K.INTERFACE].includes(detail.item.kind)" aria-label="Identity candidates"><button :disabled="findingCandidates" @click="findIdentityCandidates">Find possible duplicates in enclosing selection</button><p v-if="findingCandidates" role="status">Checking indexed source claims…</p><template v-if="identityCandidates"><p v-if="!identityCandidates.length">No comparable candidates are available in this selection. Missing evidence does not prove that there are no duplicates.</p><ul><li v-for="candidate in identityCandidates" :key="key(candidate.target)">{{ label(candidate.kind) }} · {{ display(candidate.fields.find(v=>v.name==='name')) }} <button @click="compareCandidate(candidate)">Compare with candidate</button></li></ul></template></section>
<button v-if="detail.item.kind === K.VIRTUAL_MACHINE && identity?.writable && detail.item.target" @click="placementVm=detail.item.target">Change hypervisor after migration</button>
<HostIdentityEvidence :observations="detail.identity" />
<ResponsibilityPanel v-if="detail.item&&!loading" :key="key(detail.item.target)+'/'+detail.item.revision" :item="detail.item" :working="working" @changed="saved()" @open="open" />
<AccountPanel v-if="detail.item?.kind===K.PROVIDER_ACCOUNT&&!loading" :key="key(detail.item.target)+'/'+detail.item.revision" :item="detail.item" :working="working" @changed="saved()" @open="open" @share="editor='view'" />
<p v-if="detail.networkNext || networkCursor" class="hint">Showing one bounded evidence page. Observation counts and filters apply to this page.</p>
<HostNetwork v-for="evidence in detail.hostNetwork" :key="evidence.sourceId" :evidence="evidence" @open="open" />
<div v-if="detail.networkNext || networkCursor" class="actions"><button v-if="networkCursor" :disabled="networkLoading" @click="networkEvidence()">First evidence page</button><button v-if="detail.networkNext" :disabled="networkLoading" @click="networkEvidence(detail.networkNext)">Next evidence page</button></div>
<div class="section-title"><h3>Typed relationships</h3><button v-if="identity?.writable" @click="relations">Connections in broader scope</button></div><p v-if="!detail.relations.length" class="hint">No relationships visible in this scope. This does not establish absence outside it.</p><ul><li v-for="r in detail.relations" :key="r.relationId"><button @click="open(key(r.from) === key(detail.item?.target) ? r.to : r.from)">{{ RelationKind[r.kind]?.replace('RELATION_KIND_', '').toLowerCase().replaceAll('_',' ') }} · {{ relatedName(key(r.from) === key(detail.item?.target) ? r.to : r.from) }}</button><button v-if="identity?.writable" @click="removeRelation(r)">Remove relationship</button></li></ul>
<button v-if="detail.relationsNext" @click="nextRelations">Next relationships</button>
<template v-for="section in [{ title: 'Observed evidence', items: detail.observed }, { title: 'Import proposals', items: detail.imported }]" :key="section.title"><h3>{{ section.title }}</h3><p v-if="!section.items.length" class="hint">{{ detail.hostNetwork.length && section.title === 'Observed evidence' ? 'No additional visible field evidence.' : 'No visible current evidence. Accepted values are not observations.' }}</p><div v-for="o in section.items" :key="o.evidence?.sourceId"><p class="mono">Source {{ o.evidence?.sourceId }}</p><dl><template v-for="f in o.evidence?.fields" :key="f.name"><dt>{{ f.name }}</dt><dd>{{ display(f) }}</dd></template></dl></div></template></section>
</div></main></div>
<AssetEditor v-if="editor === 'asset'" :item="editing" :initial-kind="initialKind" :initial-hypervisor="query.view === model.View.VIRTUALIZATION && query.kinds.length === 1 && query.kinds[0] === K.SOFTWARE_INSTALLATION" :definitions="definitions" :working="working" :view="query.view" @close="editor = ''" @saved="saved" />
<SchemaEditor v-if="editor === 'schema'" :definitions="definitions" :revision="page.revision" @close="editor = ''" @saved="saved" />
<ImportEditor v-if="editor === 'import'" :definitions="definitions" :working="working" @close="editor = ''" @saved="saved" />
<HostImportEditor v-if="editor === 'host-import'" :working="working" @close="editor = ''" @saved="saved" />
<HostIdentityCompare v-if="identityPair" :left="identityPair.left" :right="identityPair.right" :working="identityPair.working" @close="identityPair=undefined" @changed="load()" />
<VmPlacement v-if="placementVm" :target="placementVm" :working="enclosing" @close="placementVm=undefined" @saved="placementSaved" />
<ViewEditor v-if="editor === 'view' && identity" :query="query" :identity="identity" :existing="activeView" @close="editor = ''" @saved="saved" />
<NetworkEditor v-if="editor === 'network'" :item="editing" :group="editing ? detail?.group : undefined" :definitions="definitions" :working="editing ? enclosing : working" :allocations="actions.allocations" :view="query.view" @close="editor = ''" @saved="saved" @refresh="load" />
<MemberEditor v-if="editor === 'member' && detail" :detail="detail" :working="enclosing" :removal="participantRemoval" @close="editor = ''; participantRemoval = undefined" @saved="participantRemoval = undefined; saved()" />
<FilterEditor v-if="editor === 'filter'" :definitions="definitions" :predicates="query.predicates" :responsibility="query.responsibility" @close="editor = ''" @apply="filters" />
<BulkEditor v-if="editor === 'bulk'" :items="selectedItems" :revision="page.revision" :definitions="definitions" :working="working" @close="editor = ''" @saved="saved" />
<RelationEditor v-if="editor === 'relation' && detail?.item?.target" :target="detail.item.target" :working="enclosing" @close="editor = ''" @saved="saved" />
<AddressBinding v-if="editor === 'binding' && detail?.item" :item="detail.item" :working="enclosing" :definitions="definitions" @close="editor = ''" @saved="saved" />
<SelectionEditor v-if="editor === 'kinds'" title="Resource kinds" :options="kindOptions" :selected="query.kinds.map(String)" :allow-empty="true" @close="editor = ''" @apply="values => { editor = ''; change({ kind: values }); }" />
<SelectionEditor v-if="editor === 'columns'" title="Columns" :options="columnOptions" :selected="query.columns" :allow-empty="false" @close="editor = ''" @apply="values => { editor = ''; change({ columns: values.join(',') }); }" />
</template>
