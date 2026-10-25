<script setup lang="ts">
import { ref, computed, watch, onMounted } from 'vue';
import { useRoute, useRouter } from 'vue-router';
import AssetEditor from './AssetEditor.vue'; import SchemaEditor from './SchemaEditor.vue'; import ImportEditor from './ImportEditor.vue'; import ViewEditor from './ViewEditor.vue'; import NetworkEditor from './NetworkEditor.vue'; import Dialog from './Dialog.vue';
import MemberEditor from './MemberEditor.vue';
import FilterEditor from './FilterEditor.vue';
import { call, model, label, display, key, kinds, AssetKind as K, RelationKind, NullValue, text, type AssetRef } from './api';
const router = useRouter(); const route = useRoute();
const identity = ref<model.Identity>(); const definitions = ref<model.Definition[]>([]); const views = ref<model.SavedView[]>([]);
const page = ref(model.Page.create()); const detail = ref<model.Detail>(); const loading = ref(false); const error = ref(''); const notice = ref('');
const editor = ref<'' | 'asset' | 'schema' | 'import' | 'view' | 'network' | 'bulk' | 'relation' | 'member' | 'filter'>(''); const editing = ref<model.Item>();
const selected = ref<string[]>([]); const search = ref(''); const relationTo = ref(''); const relationKind = ref(RelationKind.INSTALLED_IN); const relationCandidates = ref<model.Item[]>([]);
const bulkName = ref('description'); const bulkValue = ref(''); const bulkMode = ref('set');
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
  view: Number(route.query.view ?? 0), search: String(route.query.search ?? ''),
  kinds: parameters('kind').map(Number), columns: String(route.query.columns ?? 'name,description').split(','),
  sort: String(route.query.sort ?? 'name'), descending: route.query.desc === '1', unplaced: route.query.unplaced === '1', limit: 25,
  cursor: typeof route.query.cursor === 'string' ? route.query.cursor : undefined,
  roots: roots(),
  predicates: parsePredicates(),
}));
function parsePredicates(): model.Predicate[] {
  if (!route.query.filter) return [];
  try { const json = JSON.parse(String(route.query.filter)); return model.Query.fromJson({ predicates: json }).predicates; } catch { return [{ field: 'unavailable', value: text('unavailable', '') }]; }
}
const activeView = computed(() => views.value.find(v => v.id === route.query.saved));
const currentTitle = computed(() => presentations.find(v => v.id === query.value.view)?.name ?? 'Inventory');
const columnLabels = computed(() => query.value.columns.map(id => ({ id, name: definitions.value.find(d => d.id === id)?.name ?? id.replaceAll('_', ' ') })));
const selectedItems = computed(() => page.value.items.filter(i => selected.value.includes(key(i.target))));
const detailName = computed(() => display(detail.value?.item?.fields.find(f => f.name === 'name')));
let generation = 0;
async function load() {
  const turn = ++generation; loading.value = true; error.value = ''; selected.value = []; search.value = query.value.search;
  try {
    const result = await call({ oneofKind: 'query', query: query.value });
    if (turn !== generation) return;
    if (result.oneofKind !== 'page') throw Error('Unexpected inventory response');
    page.value = result.page;
    if (route.query.asset && route.query.incarnation) await read({ assetId: String(route.query.asset), incarnation: String(route.query.incarnation) }); else detail.value = undefined;
  } catch (e) { if (turn === generation) { page.value = model.Page.create(); detail.value = undefined; error.value = String(e); } } finally { if (turn === generation) loading.value = false; }
}
async function metadata() {
  const [i, d, v] = await Promise.all([call({ oneofKind: 'identity', identity: {} }), call({ oneofKind: 'definitions', definitions: {} }), call({ oneofKind: 'views', views: {} })]);
  if (i.oneofKind === 'identity') identity.value = i.identity; if (d.oneofKind === 'definitions') definitions.value = d.definitions.items; if (v.oneofKind === 'views') views.value = v.views.items;
}
async function read(target: AssetRef) {
  const result = await call({ oneofKind: 'read', read: { target } });
  if (result.oneofKind === 'detail') detail.value = result.detail;
}
async function change(values: Record<string, string | string[] | undefined>) { await router.push({ path: '/', query: { ...route.query, cursor: undefined, ...values } }); }
async function selectedScope() {
  const targets = selectedItems.value.flatMap(i => i.target ? [i.target] : []);
  await change({ scope: targets.map(t => t.assetId), scopeInc: targets.map(t => t.incarnation), asset: undefined, incarnation: undefined });
}
async function filters(predicates: model.Predicate[]) {
  const json = model.Query.toJson(model.Query.create({ predicates }));
  if (!json || typeof json !== 'object' || Array.isArray(json)) throw Error('Invalid query definition');
  editor.value = ''; await change({ filter: predicates.length ? JSON.stringify(json.predicates) : undefined });
}
async function open(target: AssetRef | undefined) { if (target) await change({ asset: target.assetId, incarnation: target.incarnation }); }
async function saved() { editor.value = ''; notice.value = 'Changes saved to local storage.'; await metadata(); await load(); }
function create() { editing.value = undefined; editor.value = 'asset'; }
function edit() { editing.value = detail.value?.item; editor.value = 'asset'; }
async function useView(v: model.SavedView) {
  if (!v.query) return;
  const q = v.query;
  const json = model.Query.toJson(q);
  const filter = q.predicates.length && json && typeof json === 'object' && !Array.isArray(json) ? JSON.stringify(json.predicates) : undefined;
  await router.push({ path: '/', query: { view: String(q.view), search: q.search || undefined, columns: q.columns.join(','), sort: q.sort, desc: q.descending ? '1' : undefined, unplaced: q.unplaced ? '1' : undefined, kind: q.kinds.map(String), scope: q.roots.map(t => t.assetId), scopeInc: q.roots.map(t => t.incarnation), filter, saved: v.id } });
}
async function exportCsv() {
  try {
    const result = await call({ oneofKind: 'export', export: { query: query.value, spreadsheet: true } });
    if (result.oneofKind !== 'download') return;
    const url = URL.createObjectURL(new Blob([result.download.csv], { type: 'text/csv;charset=utf-8' }));
    const link = document.createElement('a'); link.href = url; link.download = 'omloom-inventory.csv'; link.click(); URL.revokeObjectURL(url);
  } catch (e) { error.value = String(e); }
}
async function bulk(remove = false) {
  try {
    await call({ oneofKind: 'bulk', bulk: { targets: selectedItems.value.flatMap(i => i.target ? [i.target] : []), expected: page.value.revision, remove,
      changes: remove ? [] : [{ name: bulkName.value, value: bulkMode.value === 'clear' ? { oneofKind: 'clear', clear: NullValue.NULL_VALUE } : { oneofKind: 'text', text: bulkValue.value } }] } }); await saved();
  } catch (e) { error.value = String(e); }
}
async function relations() {
  try { const r = await call({ oneofKind: 'query', query: model.Query.create({ limit: 100, columns: ['name'] }) }); if (r.oneofKind === 'page') relationCandidates.value = r.page.items; editor.value = 'relation'; } catch(e) { error.value = String(e); }
}
async function addRelation() {
  try {
    const destination = relationCandidates.value.find(i => key(i.target) === relationTo.value)?.target;
    await call({ oneofKind: 'relation', relation: { relation: { relationId: '', from: detail.value?.item?.target, to: destination, kind: relationKind.value, revision: 0n, authorityId: '' }, remove: false } }); await saved();
  } catch (e) { error.value = String(e); }
}
async function removeParticipant(participant: model.Participant) {
  try {
    await call({ oneofKind: 'member', member: model.Member.create({ address: detail.value?.item?.target, expected: detail.value?.item?.revision, interface: participant.interface, remove: true }) }); await saved();
  } catch (e) { error.value = String(e); }
}
async function removeRelation(relation: import('./api').Relation) {
  try { await call({ oneofKind: 'relation', relation: { relation, remove: true } }); await saved(); } catch (e) { error.value = String(e); }
}
function allocate() { editing.value = undefined; editor.value = 'network'; }
watch(() => route.fullPath, load);
onMounted(async () => { try { await metadata(); await load(); } catch (e) { error.value = String(e); } });
</script>
<template><div class="shell"><aside class="sidebar"><a class="brand" href="#/">omloom<span>Infrastructure workspace</span></a><p class="nav-caption">WORKSPACE</p><nav aria-label="Views"><button v-for="v in presentations" :key="v.id" :class="{ active: query.view === v.id }" @click="change({ view: String(v.id), kind: undefined })"><span aria-hidden="true">{{ v.symbol }}</span>{{ v.name }}</button></nav>
<div class="saved-heading"><p class="nav-caption">SAVED VIEWS</p><button aria-label="Save current view" @click="editor = 'view'">+</button></div><button v-for="v in views" :key="v.id" class="saved-link" @click="useView(v)">{{ v.audiences.length ? '♧' : '◦' }} {{ v.name }}</button><p v-if="!views.length" class="sidebar-hint">Keep useful selections here.</p>
<div class="fixture"><span class="status-dot" />Local development<strong>{{ identity?.name ?? 'Connecting…' }}</strong><small>Fixture scope: {{ identity?.fixture }}</small></div></aside>
<main><header class="topbar"><div class="breadcrumb">Workspace <span>/</span> {{ currentTitle }}</div><button @click="load" :disabled="loading">↻ Refresh</button></header>
<div class="workspace-head"><div><p class="eyebrow">CURRENT INVENTORY</p><h1>{{ currentTitle }}</h1><p class="subtitle">Explore infrastructure in the context that matters.</p></div><div class="actions"><button @click="exportCsv">Export CSV</button><button v-if="identity?.writable" @click="editor = 'import'">Import CSV</button><button v-if="identity?.writable" class="primary" @click="create">+ Create resource</button></div></div>
<div class="scopebar"><span class="scope-label">SCOPE</span><button v-if="query.roots.length" @click="change({ scope: undefined, scopeInc: undefined })">{{ query.roots.length }} selected roots + permitted descendants <span aria-hidden="true">×</span></button><span v-else>All permitted infrastructure</span><small>Changing views preserves this scope.</small></div>
<p v-if="notice" class="notice" role="status">{{ notice }}<button aria-label="Dismiss notice" @click="notice = ''">×</button></p><p v-if="error" class="error" role="alert">{{ error }}<button @click="change({ cursor: undefined })">Reload selection</button></p>
<section class="inventory" aria-label="Inventory"><div class="filters"><form class="search" @submit.prevent="change({ search: search || undefined })"><span aria-hidden="true">⌕</span><input v-model="search" aria-label="Search inventory" placeholder="Search names, identifiers, selected descriptions…" maxlength="256"><button>Search</button></form><select aria-label="Resource kind filter" :value="query.kinds[0] ?? ''" @change="change({ kind: ($event.target as HTMLSelectElement).value || undefined })"><option value="">All resource kinds</option><option v-for="k in kinds" :key="k" :value="k">{{ label(k) }}</option></select><label class="check"><input :checked="query.unplaced" type="checkbox" @change="change({ unplaced: ($event.target as HTMLInputElement).checked ? '1' : undefined })">Unplaced</label><button v-if="identity?.schemaAdmin" @click="editor = 'schema'">Company fields</button></div>
<div class="table-meta"><span>{{ page.total.toString() }} resources<span v-if="loading"> · Loading…</span></span><div><label>Columns<select multiple aria-label="Visible columns" :value="query.columns" @change="change({ columns: Array.from(($event.target as HTMLSelectElement).selectedOptions, o => o.value).join(',') || 'name' })"><option v-for="f in [{id:'name',name:'Name'},{id:'description',name:'Description'},{id:'serial',name:'Serial number'},...definitions.filter(d => !d.retired)]" :key="f.id" :value="f.id">{{ f.name }}</option></select></label><button @click="editor = 'view'">Save view</button><button v-if="activeView" @click="useView(activeView)">Reset adjustments</button><button v-if="identity?.writable && query.view === model.View.NETWORK" @click="allocate">Allocate IP / prefix</button><button @click="editor = 'filter'">Add filter</button></div></div>
<div v-if="query.predicates.length" class="selection"><span v-for="(p, index) in query.predicates" :key="index" class="chip">{{ definitions.find(d => d.id === p.field)?.name ?? p.field }} = {{ display(p.value) }}<button :aria-label="`Remove filter ${index + 1}`" @click="filters(query.predicates.filter((_, i) => i !== index))">×</button></span></div>
<div v-if="selected.length" class="selection">{{ selected.length }} selected on this page<button @click="selectedScope">Use selected as scope</button><button v-if="identity?.writable" @click="editor = 'bulk'">Review bulk change</button><button @click="selected = []">Clear selection</button></div>
<div class="table-scroll"><table><thead><tr><th class="select-col"><input aria-label="Select visible page" type="checkbox" :checked="!!page.items.length && selected.length === page.items.length" @change="selected = ($event.target as HTMLInputElement).checked ? page.items.map(i => key(i.target)) : []"></th><th>Resource kind</th><th v-for="c in columnLabels" :key="c.id"><button @click="change({ sort: c.id === 'name' ? 'name' : 'kind', desc: query.descending ? undefined : '1' })">{{ c.name }} <span v-if="(c.id === 'name' && query.sort === 'name')">{{ query.descending ? '↓' : '↑' }}</span></button></th><th>Identity</th></tr></thead><tbody><tr v-for="i in page.items" :key="key(i.target)" :class="{ focused: key(i.target) === key(detail?.item?.target) }"><td><input v-model="selected" type="checkbox" :value="key(i.target)" :aria-label="`Select ${display(i.fields.find(f => f.name === 'name'))}`"></td><td><span class="kind-tag">{{ label(i.kind) }}</span></td><td v-for="c in columnLabels" :key="c.id"><button v-if="c.id === 'name'" class="resource-link" @click="open(i.target)">{{ display(i.fields.find(f => f.name === c.id)) || 'Unnamed resource' }}</button><span v-else>{{ display(i.fields.find(f => f.name === c.id)) || '·' }}</span></td><td><button class="identity-link" @click="open(i.target)">{{ i.target?.assetId.slice(-8) }} ↗</button></td></tr></tbody></table></div>
<div v-if="!page.items.length && !loading && !error" class="empty"><span aria-hidden="true">▦</span><h2>{{ query.search || query.kinds.length || query.roots.length ? 'No resources match this selection' : 'Start with the infrastructure you know' }}</h2><p>{{ query.search ? 'Try another term or change the scope. Search only includes fields you may read.' : 'Create a resource or preview a CSV import. Each object keeps its own stable identity.' }}</p><button v-if="identity?.writable" class="primary" @click="create">Create resource</button></div>
<footer class="pagination"><span>Current accepted state · revision {{ page.revision.toString() }}</span><div><button v-if="query.cursor" @click="change({ cursor: undefined })">First page</button><button :disabled="!page.next" @click="change({ cursor: page.next })">Next page →</button></div></footer></section>
<section v-if="detail?.item" class="detail" aria-label="Resource details"><header><div><p class="eyebrow">{{ label(detail.item.kind) }}</p><h2>{{ detailName || 'Resource details' }}</h2></div><div class="actions"><button @click="change({ scope: detail.item.target?.assetId, scopeInc: detail.item.target?.incarnation, asset: undefined, incarnation: undefined })">Use as scope</button><button v-if="identity?.writable" @click="edit">Edit resource</button><button aria-label="Close resource details" @click="change({ asset: undefined, incarnation: undefined })">×</button></div></header><dl><template v-for="f in detail.item.fields" :key="f.name"><dt>{{ definitions.find(d => d.id === f.name)?.name ?? f.name.replaceAll('_',' ') }}</dt><dd>{{ display(f) }}</dd></template><dt>Stable identity</dt><dd class="mono">{{ detail.item.target?.assetId }}</dd><dt>Incarnation</dt><dd class="mono">{{ detail.item.target?.incarnation }}</dd><dt>Accepted revision</dt><dd>{{ detail.item.revision.toString() }}</dd></dl>
<section v-if="[K.IP_ADDRESS, K.PREFIX].includes(detail.item.kind)"><div class="section-title"><h3>Address allocation</h3><button v-if="identity?.writable" @click="editing = detail.item; editor = 'network'">Review network settings</button><button v-if="identity?.writable && detail.item.kind === K.IP_ADDRESS" @click="editor = 'member'">Assign participant</button></div><p v-if="!detail.participantsComplete" class="hint">Participant coverage is restricted. Missing participants do not prove the allocation is free.</p><ul><li v-for="p in detail.participants" :key="key(p.interface)"><button @click="open(p.interface)">Interface {{ p.interface?.assetId.slice(-8) }}</button><span v-if="p.priority !== undefined"> · Intended priority {{ p.priority }}</span><button v-if="identity?.writable" @click="removeParticipant(p)">Remove participant</button></li></ul></section>
<div class="section-title"><h3>Typed relationships</h3><button v-if="identity?.writable" @click="relations">Add relationship</button></div><p v-if="!detail.relations.length" class="hint">No relationships visible in this scope. This does not establish absence outside it.</p><ul><li v-for="r in detail.relations" :key="r.relationId"><button @click="open(key(r.from) === key(detail.item?.target) ? r.to : r.from)">{{ RelationKind[r.kind]?.replace('RELATION_KIND_', '').toLowerCase().replaceAll('_',' ') }} · {{ (key(r.from) === key(detail.item?.target) ? r.to : r.from)?.assetId.slice(-8) }}</button><button v-if="identity?.writable" @click="removeRelation(r)">Remove relationship</button></li></ul>
<template v-for="section in [{ title: 'Observed evidence', items: detail.observed }, { title: 'Import proposals', items: detail.imported }]" :key="section.title"><h3>{{ section.title }}</h3><p v-if="!section.items.length" class="hint">No visible current evidence. Accepted values are not observations.</p><div v-for="o in section.items" :key="o.evidence?.sourceId"><p class="mono">Source {{ o.evidence?.sourceId }}</p><dl><template v-for="f in o.evidence?.fields" :key="f.name"><dt>{{ f.name }}</dt><dd>{{ display(f) }}</dd></template></dl></div></template></section>
</main></div>
<AssetEditor v-if="editor === 'asset'" :item="editing" :definitions="definitions" @close="editor = ''" @saved="saved" />
<SchemaEditor v-if="editor === 'schema'" :definitions="definitions" :revision="page.revision" @close="editor = ''" @saved="saved" />
<ImportEditor v-if="editor === 'import'" :definitions="definitions" @close="editor = ''" @saved="saved" />
<ViewEditor v-if="editor === 'view' && identity" :query="query" :identity="identity" :existing="activeView" @close="editor = ''" @saved="saved" />
<NetworkEditor v-if="editor === 'network'" :item="editing" @close="editor = ''" @saved="saved" />
<MemberEditor v-if="editor === 'member' && detail" :detail="detail" @close="editor = ''" @saved="saved" />
<FilterEditor v-if="editor === 'filter'" :definitions="definitions" :predicates="query.predicates" @close="editor = ''" @apply="filters" />
<Dialog v-if="editor === 'bulk'" title="Review bulk change" @close="editor = ''"><form @submit.prevent="bulk(false)"><p>Apply only to these {{ selectedItems.length }} reviewed identities, at revision {{ page.revision }}.</p><ul><li v-for="i in selectedItems" :key="key(i.target)">{{ display(i.fields.find(f => f.name === 'name')) }} · {{ i.target?.assetId }}</li></ul><label>Field<select v-model="bulkName"><option value="description">Description</option><option value="serial">Serial number</option></select></label><label>Change<select v-model="bulkMode"><option value="set">Set</option><option value="clear">Clear explicitly</option></select></label><label v-if="bulkMode === 'set'">Value<input v-model="bulkValue" maxlength="2048"></label><p v-if="error" role="alert">{{ error }}</p><footer><button type="button" @click="editor = ''">Cancel</button><button class="danger" type="button" @click="bulk(true)">Remove reviewed resources</button><button class="primary">Apply reviewed change</button></footer></form></Dialog>
<Dialog v-if="editor === 'relation'" title="Add typed relationship" @close="editor = ''"><form @submit.prevent="addRelation"><label>Relationship<select v-model="relationKind"><option v-for="r in Object.values(RelationKind).filter((r): r is number => typeof r === 'number' && r > 0)" :key="r" :value="r">{{ RelationKind[r]?.replace('RELATION_KIND_', '').toLowerCase().replaceAll('_',' ') }}</option></select></label><label>Destination<select v-model="relationTo" required><option value="">Select permitted resource</option><option v-for="i in relationCandidates.filter(i => key(i.target) !== key(detail?.item?.target))" :key="key(i.target)" :value="key(i.target)">{{ display(i.fields.find(f => f.name === 'name')) }} · {{ label(i.kind) }}</option></select></label><p class="hint">The server checks both endpoints, resource kinds and current write permissions.</p><p v-if="error" role="alert">{{ error }}</p><footer><button type="button" @click="editor = ''">Cancel</button><button class="primary">Add relationship</button></footer></form></Dialog>
</template>
