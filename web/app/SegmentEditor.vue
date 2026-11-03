<script setup lang="ts">
import { computed, onMounted, ref, watch } from 'vue';
import Dialog from './Dialog.vue';
import { call, model, key, identityName, AssetKind as K } from './api';
const props = defineProps<{ subjects: model.Item[]; membership?: model.SegmentMembership }>();
const emit = defineEmits<{ close: []; saved: [] }>();
const mode = ref(props.membership ? 'existing' : 'new');
const chosen = ref<model.Item | undefined>(props.membership?.segment);
const name = ref(''); const search = ref(''); const options = ref(model.Page.create());
const memberships = ref(model.SegmentMembershipPage.create());
const error = ref(''); const busy = ref(false);
type Participant = { subject: model.Item; interface?: model.Item; relationId?: string; options?: model.Page; search: string };
const participants = ref<Participant[]>(props.subjects.map(subject => ({ subject, search: '', interface: props.membership?.source?.kind === K.INTERFACE ? props.membership.source : undefined, relationId: props.membership?.relation?.relationId })));
const reviewed = ref<{ request: model.Segment; working: model.WorkingContext; value: model.SegmentReview }>();
const pending = ref(false);
const title = (item?: model.Item) => identityName(item);
const roots = computed(() => [...props.subjects.flatMap(i => i.target ? [i.target] : []), ...(mode.value === 'existing' && chosen.value?.target ? [chosen.value.target] : [])]);
const context = () => model.WorkingContext.create({ roots: roots.value });
const common = computed(() => {
  const groups = new Map<string, { item: model.Item; subjects: Set<string> }>();
  for (const entry of memberships.value.entries) {
    if (!entry.segment || !entry.subject) continue;
    const id = key(entry.segment.target); const group = groups.get(id) ?? { item: entry.segment, subjects: new Set<string>() };
    group.subjects.add(key(entry.subject.target)); groups.set(id, group);
  }
  return [...groups.values()].filter(g => props.subjects.every(s => g.subjects.has(key(s.target)))).map(g => g.item);
});
function invalidate() { if (!pending.value) reviewed.value = undefined; }
watch([mode, chosen, name], invalidate, { flush: 'sync' });
function choose(item: model.Item) {
  chosen.value = item; mode.value = 'existing';
  for (const participant of participants.value) {
    participant.relationId = undefined;
    const matches = memberships.value.entries.filter(e => key(e.subject?.target) === key(participant.subject.target) && key(e.segment?.target) === key(item.target));
    if (matches.length === 1) useMembership(participant, matches[0]!);
  }
}
watch(mode, value => { if (value === 'new') for (const participant of participants.value) participant.relationId = undefined; }, { flush: 'sync' });
function back() {
  if (reviewed.value && !pending.value) { reviewed.value = undefined; return; }
  emit('close');
}
async function existing(cursor?: string) {
  busy.value = true; error.value = '';
  try {
    const reply = await call({ oneofKind: 'query', query: model.Query.create({ kinds: [K.L2_DOMAIN], columns: ['name'], search: search.value, limit: 25, cursor }) });
    if (reply.oneofKind !== 'page') throw Error('Segment options are unavailable.');
    options.value = reply.page;
  } catch(e) { error.value = String(e); } finally { busy.value = false; }
}
async function loadMemberships(cursor?: string) {
  busy.value = true; error.value = '';
  try {
    const reply = await call({ oneofKind: 'segmentMemberships', segmentMemberships: model.SegmentMemberships.create({ targets: props.subjects.flatMap(i => i.target ? [i.target] : []), cursor }) }, context());
    if (reply.oneofKind !== 'segmentMembershipPage') throw Error('Current memberships are unavailable.');
    memberships.value = model.SegmentMembershipPage.create({ ...reply.segmentMembershipPage, entries: [...(cursor ? memberships.value.entries : []), ...reply.segmentMembershipPage.entries] });
  } catch(e) { error.value = String(e); } finally { busy.value = false; }
}
async function interfaces(participant: Participant, cursor?: string) {
  busy.value = true; error.value = '';
  try {
    const target = participant.subject.target; if (!target) throw Error('VM identity is missing.');
    const reply = await call({ oneofKind: 'segmentInterfaces', segmentInterfaces: model.SegmentInterfaces.create({ subject: target, search: participant.search, cursor }) }, model.WorkingContext.create({ roots: [target] }));
    if (reply.oneofKind !== 'page') throw Error('Interface choices are unavailable.'); participant.options = reply.page;
  } catch(e) { error.value = String(e); } finally { busy.value = false; }
}
function useMembership(participant: Participant, entry: model.SegmentMembership) {
  participant.interface = entry.source?.kind === K.INTERFACE ? entry.source : undefined;
  participant.relationId = entry.relation?.relationId; invalidate();
}
function interfaceHint(participant: Participant, item: model.Item) {
  const context = participant.options?.contexts.find(c => key(c.target) === key(item.target));
  return [...(context?.owners.map(o => o.name) ?? []), ...(context?.addresses ?? []), item.target?.assetId.slice(-8)].filter(Boolean).join(' · ');
}
async function preview() {
  busy.value = true; error.value = ''; reviewed.value = undefined;
  try {
    if (mode.value === 'existing' && !chosen.value?.target) throw Error('Choose an existing segment.');
    const request = model.Segment.create({ target: mode.value === 'existing' ? chosen.value?.target : undefined, name: mode.value === 'new' && name.value ? name.value : undefined, participants: participants.value.map(p => ({ subject: p.subject.target, interface: p.interface?.target, relationId: p.relationId })) });
    const working = context();
    const reply = await call({ oneofKind: 'previewSegment', previewSegment: request }, working);
    if (reply.oneofKind !== 'segmentReview') throw Error('Membership review is unavailable.');
    working.expected = reply.segmentReview.revision;
    reviewed.value = { request, working, value: reply.segmentReview };
  } catch(e) { error.value = String(e); } finally { busy.value = false; }
}
async function save() {
  if (!reviewed.value) return;
  busy.value = true; pending.value = true; error.value = '';
  try {
    const reply = await call({ oneofKind: 'segment', segment: reviewed.value.request }, reviewed.value.working);
    if (reply.oneofKind !== 'mutation') throw Error('No mutation receipt was returned.');
    emit('saved');
  } catch(e) {
    // ABORTED reports a rejected revision; transport failures leave the outcome unknown.
    if (e instanceof Error && 'code' in e && e.code === 'ABORTED') {
      pending.value = false; reviewed.value = undefined;
      error.value = 'State changed. Your draft is retained. Review participation again before saving.';
    } else {
      error.value = `${String(e)} The reviewed command is retained. Retry it before preparing a different change.`;
    }
  }
  finally { busy.value = false; }
}
onMounted(() => loadMemberships());
</script>
<template><Dialog title="Shared L2 segment" :dismissible="!busy" @close="back"><form @submit.prevent="preview">
<p>Record only the selected VMs' participation. IPs, prefixes, VLANs, routing and connectivity remain independent. Other memberships stay in place.</p>
<div v-if="!reviewed">
<fieldset :disabled="busy || pending"><legend>Segment</legend>
<label><input v-model="mode" type="radio" value="new">New distinct segment, unnamed unless you know its name</label>
<label v-if="mode === 'new'">Name (optional)<input v-model="name" maxlength="160"></label>
<label><input v-model="mode" type="radio" value="existing">Choose an existing segment</label>
<template v-if="mode === 'existing'"><p>Selected: {{ chosen ? title(chosen) : 'None' }} <small v-if="chosen">· {{chosen.target?.assetId.slice(-8)}}</small></p><label>Find segment<input v-model="search" maxlength="256"></label><button type="button" @click="existing()">Find segments</button><ul><li v-for="item in options.items" :key="key(item.target)">{{ title(item) }} <small>· {{item.target?.assetId.slice(-8)}}</small> <button type="button" @click="choose(item)">Choose this segment</button></li></ul><button v-if="options.next" type="button" @click="existing(options.next)">More segments</button></template>
<section v-if="common.length"><h3>Already shared by these VMs</h3><p>Choose explicitly. Matching labels alone never establish a common segment.</p><button v-for="item in common" :key="key(item.target)" type="button" @click="choose(item)">{{ title(item) }}</button></section>
<button v-if="memberships.next" type="button" @click="loadMemberships(memberships.next)">More current memberships</button>
</fieldset>
<fieldset v-for="participant in participants" :key="key(participant.subject.target)" :disabled="busy || pending"><legend>{{ title(participant.subject) }}</legend>
<p>Interface: {{ participant.interface ? title(participant.interface) : 'Unknown, no interface will be invented' }}</p>
<template v-if="mode === 'existing' && chosen"><label v-for="entry in memberships.entries.filter(e => key(e.subject?.target) === key(participant.subject.target) && key(e.segment?.target) === key(chosen?.target))" :key="entry.relation?.relationId"><input type="radio" :checked="participant.relationId === entry.relation?.relationId" @change="useMembership(participant, entry)">Refine this assertion: {{ title(entry.source) }} ({{entry.relation?.relationId.slice(-8)}})</label></template>
<button v-if="participant.relationId" type="button" @click="participant.relationId = undefined; participant.interface = undefined; invalidate()">Add another known interface instead</button>
<label>Find a known interface<input v-model="participant.search" maxlength="256"></label><button type="button" @click="interfaces(participant)">Find interfaces of this VM</button>
<ul><li v-for="item in participant.options?.items" :key="key(item.target)">{{ title(item) }} <small>· {{interfaceHint(participant,item)}}</small> <button type="button" @click="participant.interface = item; invalidate()">Use interface</button></li></ul><button v-if="participant.options?.next" type="button" @click="interfaces(participant, participant.options.next)">More interfaces</button>
<button v-if="participant.interface" type="button" @click="participant.interface = undefined; invalidate()">Interface is unknown</button></fieldset>
</div>
<section v-if="reviewed" aria-label="Membership summary"><h3>Review before saving</h3><p>{{ reviewed.value.segment ? title(reviewed.value.segment) : name || 'New unnamed L2 segment' }}</p><ul><li v-for="p in reviewed.value.participants" :key="key(p.subject?.target)">{{ title(p.subject) }} → {{ p.interface ? title(p.interface) : 'interface unknown' }} → selected segment<span v-if="p.relation"> · preserves assertion {{p.relation.relationId.slice(-8)}}</span></li></ul><p>This records network knowledge only. It does not configure interfaces or prove packet delivery.</p></section>
<p v-if="error" role="alert">{{ error }}</p><footer><button type="button" :disabled="busy" @click="back">{{ pending ? 'Close with outcome unresolved' : 'Back' }}</button><button v-if="!reviewed" :disabled="busy || pending">Review participation</button><button v-if="reviewed" type="button" class="primary" :disabled="busy" @click="save">{{ pending ? 'Retry reviewed command' : 'Save reviewed participation' }}</button></footer>
</form></Dialog></template>
