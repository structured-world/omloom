<script setup lang="ts">
import { computed, onMounted, ref } from 'vue';
import Dialog from './Dialog.vue';
import { call, model, key, label, display, RelationKind, type AssetRef, type Relation } from './api';
const props = defineProps<{ target: AssetRef; working: model.WorkingContext }>();
const emit = defineEmits<{ close: []; saved: [] }>();
const step = ref<'scope' | 'choose' | 'review' | 'connections'>('scope');
const bounds = ref<AssetRef[]>(props.working.roots);
const anchor = ref<model.Item>();
const search = ref(''); const next = ref<string>(); const items = ref<model.Item[]>([]);
const destination = ref(''); const chosen = ref<model.Item>();
const kinds = ref<RelationKind[]>([]); const kind = ref<RelationKind>();
const anchorFrom = ref(true); const editing = ref<Relation>(); const removing = ref(false);
const revision = ref<bigint>(); const error = ref(''); const busy = ref(false); const existing = ref<model.Detail>();
const connectionPage = ref<string>();
const name = (item: model.Item | undefined) => display(item?.fields.find(f => f.name === 'name')) || item?.target?.assetId.slice(-8);
const scope = computed(() => {
  const targets = [props.target, chosen.value?.target, editing.value?.from, editing.value?.to].filter((t): t is AssetRef => !!t);
  return [...new Map(targets.map(t => [key(t), t])).values()];
});
const source = computed(() => anchorFrom.value ? props.target : chosen.value?.target);
const target = computed(() => anchorFrom.value ? chosen.value?.target : props.target);
const relationLabel = (value: RelationKind) => RelationKind[value]!.replace('RELATION_KIND_', '').toLowerCase().replaceAll('_', ' ');
onMounted(async () => { try {
  const reply = await call({ oneofKind: 'read', read: { target: props.target } }, model.WorkingContext.create({ roots: bounds.value, focus: props.target }));
  if (reply.oneofKind !== 'detail') throw Error('Resource context is unavailable'); anchor.value = reply.detail.item;
} catch (e) { error.value = String(e); } });
async function load(cursor?: string) {
  busy.value = true; error.value = '';
  try {
    step.value = 'choose';
    const reply = await call({ oneofKind: 'query', query: model.Query.create({ roots: bounds.value, limit: 50, columns: ['name'], search: search.value, cursor }) });
    if (reply.oneofKind !== 'page') throw Error('Unexpected inventory response');
    items.value = reply.page.items.filter(i => key(i.target) !== key(props.target)); next.value = reply.page.next;
  } catch (e) { error.value = String(e); } finally { busy.value = false; }
}
function select() { chosen.value = items.value.find(i => key(i.target) === destination.value); }
async function connections(cursor?: string) {
  busy.value = true; error.value = '';
  try {
    const reply = await call({ oneofKind: 'read', read: { target: props.target, relationsCursor: cursor } }, model.WorkingContext.create({ roots: bounds.value }));
    if (reply.oneofKind !== 'detail') throw Error('Connections are unavailable'); existing.value = reply.detail; step.value = 'connections';
    connectionPage.value = cursor;
  } catch (e) { error.value = String(e); } finally { busy.value = false; }
}
async function prepare() {
  busy.value = true; error.value = '';
  try {
    if (!chosen.value?.target) throw Error('Choose the other endpoint');
    const reply = await call({ oneofKind: 'prepare', prepare: model.Prepare.create({ from: source.value, to: target.value }) }, model.WorkingContext.create({ roots: scope.value }));
    if (reply.oneofKind !== 'actions') throw Error('Connection review is unavailable');
    kinds.value = reply.actions.relationships; revision.value = reply.actions.revision;
    if (kind.value === undefined || !kinds.value.includes(kind.value)) kind.value = undefined;
    step.value = 'review';
  } catch (e) { error.value = String(e); } finally { busy.value = false; }
}
async function review(relation: Relation, remove: boolean) {
  editing.value = relation; removing.value = remove; anchorFrom.value = key(relation.from) === key(props.target);
  const other = anchorFrom.value ? relation.to : relation.from;
  chosen.value = existing.value?.related.find(i => key(i.target) === key(other));
  kind.value = relation.kind;
  if (remove) await prepare(); else { destination.value = key(other); await load(); }
}
async function save() {
  busy.value = true; error.value = '';
  try {
    if (revision.value === undefined || kind.value === undefined || !kinds.value.includes(kind.value)) throw Error('Review a supported connection first');
    await call({ oneofKind: 'relation', relation: model.Relationship.create({
      relation: { relationId: editing.value?.relationId ?? '', authorityId: '', revision: editing.value?.revision ?? 0n, from: source.value, to: target.value, kind: kind.value },
      remove: removing.value,
    }) }, model.WorkingContext.create({ roots: scope.value, expected: revision.value }));
    emit('saved');
  } catch (e) { error.value = String(e); } finally { busy.value = false; }
}
function otherName(relation: Relation) { const other = key(relation.from) === key(props.target) ? relation.to : relation.from; return name(existing.value?.related.find(i => key(i.target) === key(other))); }
function start() { editing.value = undefined; removing.value = false; chosen.value = undefined; destination.value = ''; anchorFrom.value = true; kind.value = undefined; return load(); }
</script>
<template><Dialog title="Connections in broader scope" @close="emit('close')">
<p class="context-chip">Current resource: {{ name(anchor) }}</p>
<p class="hint">Linking peers uses a context containing both resources. Each endpoint keeps its own permissions.</p>
<template v-if="step === 'scope'">
<p>{{ bounds.length ? 'Search within the enclosing selection.' : 'Choose from permitted inventory, then review only the selected endpoints.' }}</p>
<footer><button type="button" @click="connections()">Review existing connections</button><button class="primary" type="button" @click="start">Choose connection endpoints</button></footer>
</template>
<template v-else-if="step === 'connections'">
<p v-if="!existing?.relations.length">No connections are visible in this enclosing context.</p>
<ul class="connection-list"><li v-for="r in existing?.relations" :key="r.relationId"><strong>{{ otherName(r) }}</strong><span>{{ relationLabel(r.kind) }}</span><button type="button" @click="review(r,false)">Change connection</button><button type="button" @click="review(r,true)">Review removal</button></li></ul>
<footer><button type="button" @click="step = 'scope'">Back</button><button v-if="connectionPage" type="button" :disabled="busy" @click="connections()">First connections</button><button v-if="existing?.relationsNext" type="button" :disabled="busy" @click="connections(existing.relationsNext)">Next connections</button><button type="button" @click="start">Add connection</button></footer>
</template>
<template v-else-if="step === 'choose'">
<p class="context-chip">{{ bounds.length ? 'Enclosing selection: '+bounds.length+' roots' : 'All permitted inventory' }}</p>
<button v-if="bounds.length" type="button" @click="bounds = []; load()">Choose across permitted inventory</button>
<form @submit.prevent="load()"><label>Find the other resource<input v-model="search" maxlength="256"></label><button :disabled="busy">Find resources</button></form>
<label for="connection-peer">Other endpoint</label><select id="connection-peer" v-model="destination" @change="select"><option value="">Choose a permitted resource</option><option v-for="i in items" :key="key(i.target)" :value="key(i.target)">{{ name(i) }} · {{ label(i.kind) }}</option></select>
<p v-if="chosen">Selected: {{ name(chosen) }}</p><button v-if="next" type="button" @click="load(next)">Next resources</button>
<footer><button type="button" @click="step = 'scope'">Back</button><button class="primary" type="button" :disabled="!chosen || busy" @click="prepare">Review connection</button></footer>
</template>
<form v-else @submit.prevent="save">
<p class="context-chip">Connection context: {{ name(anchor) }} + {{ name(chosen) }}</p>
<p v-if="editing" class="hint">The review includes both the previous and proposed endpoints.</p>
<dl><dt>From</dt><dd>{{ anchorFrom ? name(anchor) : name(chosen) }}</dd><dt>To</dt><dd>{{ anchorFrom ? name(chosen) : name(anchor) }}</dd></dl>
<button v-if="!removing" type="button" @click="anchorFrom = !anchorFrom; prepare()">Reverse direction</button>
<label for="connection-kind">Relationship</label><select id="connection-kind" v-model="kind" :disabled="removing" required><option :value="undefined" disabled>Choose the relationship meaning</option><option v-for="value in kinds" :key="value" :value="value">{{ relationLabel(value) }}</option></select>
<p v-if="!kinds.length" role="status">No supported relationship in this direction. Review the opposite direction or choose another endpoint.</p>
<p v-if="removing">Remove this relationship. Both resources remain in inventory.</p>
<footer><button type="button" @click="removing ? connections() : step = 'choose'">Back</button><button class="primary" :disabled="busy || kind === undefined || !kinds.length">{{ removing ? 'Remove reviewed relationship' : editing ? 'Save reviewed relationship' : 'Add relationship' }}</button></footer>
</form>
<p v-if="busy" role="status">Checking current context…</p><p v-if="error" role="alert">{{ error }}</p>
<footer><button type="button" @click="emit('close')">Return to resource</button></footer>
</Dialog></template>
