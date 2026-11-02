<script setup lang="ts">
import { ref, computed, onMounted } from 'vue';
import Dialog from './Dialog.vue';
import HostIdentityEvidence from './HostIdentityEvidence.vue';
import { call, model, display, identityName as name, label, key, AssetKind as K, RelationKind as R, type AssetRef, type Field } from './api';
const props = defineProps<{ left: AssetRef; right: AssetRef; working: model.WorkingContext }>();
const emit = defineEmits<{ close: []; changed: [] }>();
const comparison = ref<model.HostIdentityComparison>();
const error = ref('');
const busy = ref(false);
const step = ref<'compare'|'decision'|'mapping'|'preview'|'receipt'>('compare');
const decision = ref(model.IdentityDecision.UNSPECIFIED);
type Draft = { pair: model.HostIdentityComparison; keep?: 'left'|'right'; fields: Record<string,string>; sources: Record<string,string>; attachments: string[] };
const drafts=ref<Draft[]>([]);
const plan=ref<model.IdentityCorrectionView>();const receipt=ref<model.IdentityCorrectionApplied>();
const layer=ref(K.SOFTWARE_INSTALLATION);const layerSearch=ref('');const layerPage=ref(model.Page.create());
const layerLeft=ref<model.Item>();const layerRight=ref<model.Item>();
const physical=(r:model.IdentityImpact['before'])=>r && [R.HAS_PORT,R.CARRIES,R.INSTALLED_IN,R.CONNECTED_TO,R.ON].includes(r.kind);
const duplicate=(draft:Draft)=>draft.keep==='left'?draft.pair.right:draft.pair.left;
const retained=(draft:Draft)=>draft.keep==='left'?draft.pair.left:draft.pair.right;
function domainDisplay(subject: model.HostIdentitySubject | undefined, field: Field) {
  const label = subject?.domainLabels.find(value => value.name === field.name);
  if (!label || field.value.oneofKind !== 'text') return display(field);
  const reference = field.value.text;
  return `${display(label)} (${reference.slice(0, reference.indexOf('/')).slice(-8)})`;
}
function prefixChanges(draft: Draft) {
  const destination = retained(draft); const duplicateSubject = duplicate(draft);
  const before = destination?.domainFields ?? [];
  const source = duplicateSubject?.domainFields ?? [];
  return ([['cidr', 'CIDR'], ['address_space', 'Address space'], ['link_scope', 'L2 domain'], ['routing_context', 'Routing context']] as const).map(([id, label]) => {
    const previous = before.find(field => field.name === id);
    // A successful preview checks compatibility; domain facts only fill missing values.
    const after = previous ?? source.find(field => field.name === id);
    const afterSubject = previous ? destination : duplicateSubject;
    return { id, label, before: previous, after,
      beforeLabel: previous ? domainDisplay(destination, previous) : 'Unknown',
      afterLabel: after ? domainDisplay(afterSubject, after) : 'Unknown',
      named: destination?.domainLabels.some(field => field.name === id) || afterSubject?.domainLabels.some(field => field.name === id),
    };
  });
}
function configure(draft:Draft) {
  draft.fields={};draft.sources={};draft.attachments=[];
  for(const source of duplicate(draft)?.sources??[])draft.sources[source]=source;
  plan.value=undefined;
}
async function expanded(left:AssetRef,right:AssetRef,includeRelations:boolean) {
  const value=await call({oneofKind:'compareHostIdentity',compareHostIdentity:model.CompareHostIdentity.create({left,right,includeRelations})},model.WorkingContext.create({...props.working,expected:undefined}));
  if(value.oneofKind!=='hostIdentityComparison')throw Error('Comparison is unavailable.');return value.hostIdentityComparison;
}
async function start() {
  busy.value=true;error.value='';
  try {
    if(!comparison.value?.left?.item)throw Error('Comparison is unavailable.');
    // Prefix membership is checked by the correction owner; this form only consumes physical attachment choices.
    if(!drafts.value.length)drafts.value=[{pair:await expanded(props.left,props.right,comparison.value.left.item.kind!==K.PREFIX),fields:{},sources:{},attachments:[]}];
    step.value='decision';
  }catch(e){error.value=String(e);}finally{busy.value=false;}
}
async function layers(cursor?:string) {
  busy.value=true;error.value='';
  try {
    const roots=drafts.value.flatMap(v=>[v.pair.left?.item?.target,v.pair.right?.item?.target]).filter((v):v is AssetRef=>!!v);
    const response=await call({oneofKind:'query',query:model.Query.create({roots,kinds:[layer.value],columns:['name'],search:layerSearch.value,limit:25,cursor,includeContext:true,levelOnly:true})});
    if(response.oneofKind!=='page')throw Error('Layer choices are unavailable.');layerPage.value=response.page;
  }catch(e){error.value=String(e);}finally{busy.value=false;}
}
async function addLayer() {
  if(!layerLeft.value?.target||!layerRight.value?.target)return;
  busy.value=true;error.value='';
  try {
    if(drafts.value.length===8)throw Error('A correction supports at most eight explicit layer mappings.');
    if(key(layerLeft.value.target)===key(layerRight.value.target))throw Error('Choose two different resources.');
    const draft:Draft={pair:await expanded(layerLeft.value.target,layerRight.value.target,true),keep:'right',fields:{},sources:{},attachments:[]};configure(draft);drafts.value.push(draft);
    layerLeft.value=undefined;layerRight.value=undefined;layerPage.value=model.Page.create();
  }catch(e){error.value=String(e);}finally{busy.value=false;}
}
async function prepare() {
  busy.value=true;error.value='';
  try {
    const mappings=drafts.value.map(draft=>{
      const source=duplicate(draft);const destination=retained(draft);
      if(!draft.keep||!source?.item?.target||!destination?.item?.target)throw Error('Choose the retained identity at every layer.');
      const fields=source.item.fields.map(field=>{const selected=draft.fields[field.name];if(!selected)throw Error(`Review the ${field.name} field.`);return {name:field.name,useDuplicate:selected==='duplicate'};});
      return model.IdentityMapping.create({duplicate:source.item.target,retained:destination.item.target,fields,sources:Object.entries(draft.sources).map(([before,after])=>({before,after})),attachments:draft.attachments});
    });
    const value=await call({oneofKind:'correctIdentity',correctIdentity:model.IdentityCorrection.create({decision:decision.value,mappings})},model.WorkingContext.create({...props.working,expected:drafts.value[0]?.pair.revision}));
    if(value.oneofKind!=='identityCorrection')throw Error('Correction preview is unavailable.');plan.value=value.identityCorrection;for(const draft of drafts.value)draft.pair.revision=value.identityCorrection.revision;step.value='preview';
  }catch(e){error.value=`${String(e)} Resolve source, parent, responsibility or external-binding conflicts before retrying. No correction was committed.`;}finally{busy.value=false;}
}
async function commit(undo=false) {
  busy.value=true;error.value='';
  try {
    const request=undo?model.IdentityCorrection.create({undo:receipt.value?.id}):model.IdentityCorrection.create({preview:plan.value?.id});
    const value=await call({oneofKind:'correctIdentity',correctIdentity:request},model.WorkingContext.create({...props.working,expected:undo?receipt.value?.revision:plan.value?.revision}));
    if(value.oneofKind!=='identityCorrected')throw Error('Correction receipt is unavailable.');receipt.value=value.identityCorrected;step.value='receipt';emit('changed');
  }catch(e){error.value=String(e);}finally{busy.value=false;}
}
const matches = computed(() => comparison.value?.left?.identity.filter(a => a.claim && comparison.value?.right?.identity.some(b => b.claim && a.claim?.kind === b.claim.kind && a.claim.namespace === b.claim.namespace && a.claim.digest === b.claim.digest)) ?? []);
async function load() {
  busy.value = true; error.value = ''; comparison.value = undefined;
  try {
    const response = await call({ oneofKind: 'compareHostIdentity', compareHostIdentity: model.CompareHostIdentity.create({ left: props.left, right: props.right }) }, model.WorkingContext.create({ ...props.working, expected: undefined }));
    if (response.oneofKind !== 'hostIdentityComparison') throw Error('Identity comparison is unavailable.');
    comparison.value = response.hostIdentityComparison;
  } catch (e) { error.value = String(e); }
  finally { busy.value = false; }
}
onMounted(load);
</script>
<template><Dialog title="Compare identity" @close="!busy && emit('close')"><div class="identity-review">
<template v-if="step==='compare'">
<p v-if="comparison?.left?.item?.kind !== K.PREFIX">Compare hardware and installed operating systems separately. Different IPs do not prove different machines. Matching identifiers may be cloned, movable or reported by the same underlying source.</p>
<p v-if="busy" role="status">Loading the two selected resources…</p>
<p v-if="error" role="alert">{{ error }}</p>
<template v-if="comparison">
<p>Current comparison at revision {{ comparison.revision.toString() }}. This read changes no inventory, grants or connections.</p>
<p v-if="matches.length">Some identifiers match within the same layer and namespace. This supports review, not automatic merging or authenticated target verification.</p>
<p v-else-if="comparison.left?.item?.kind === K.PREFIX">Compare the declared CIDR, namespace and relationships. Equal address text alone does not establish the same network.</p>
<p v-else>No matching identity claims are available in this comparison. Missing evidence does not prove that these are different machines.</p>
<div class="identity-pair"><section v-for="(subject, index) in [comparison.left, comparison.right]" :key="index" :aria-label="index === 0 ? 'First identity' : 'Second identity'">
<template v-if="subject?.item"><h3>{{ name(subject.item, subject.domainFields) }}</h3>
<p>{{ label(subject.item.kind) }}</p><dl><template v-for="field in [...subject.item.fields.filter(v => v.name !== 'name'), ...subject.domainFields]" :key="field.name"><dt>{{ field.name.replaceAll('_', ' ') }}</dt><dd>{{ domainDisplay(subject, field) }}</dd></template></dl>
<HostIdentityEvidence :observations="subject.identity" />
<p v-if="!subject.identity.length && subject.item.kind !== K.PREFIX">No identity evidence is available to this view. Do not infer hardware or installation continuity from the name.</p>
</template></section></div></template>
<footer><button @click="emit('close')">Back</button><button v-if="error" :disabled="busy" @click="load">Retry comparison</button><button :disabled="busy||!comparison" @click="start">Plan identity correction</button></footer>
</template>
<template v-else-if="step==='decision'">
<h3>What do these records represent?</h3><p>Identifiers support review; they cannot establish that these records describe the same resource.</p>
<label><input v-model="decision" type="radio" :value="model.IdentityDecision.KEEP_SEPARATE">Different or not established: keep separate</label>
<label v-if="comparison?.left?.item?.kind !== K.PREFIX"><input v-model="decision" type="radio" :value="model.IdentityDecision.DISTINCT_INSTALLATIONS">Same hardware, different OS installations</label>
<label v-if="comparison?.left?.item?.kind !== K.PREFIX"><input v-model="decision" type="radio" :value="model.IdentityDecision.DUPLICATE_INSTALLATION">Duplicate records of the same hardware and installation</label>
<label v-if="comparison?.left?.item?.kind === K.PREFIX"><input v-model="decision" type="radio" :value="model.IdentityDecision.DUPLICATE_RESOURCE">Duplicate records of the same prefix</label>
<p v-if="comparison?.left?.item?.kind !== K.PREFIX">Separate OS installations retain their own services, interfaces and addresses. Duplicate installations require an explicit mapping at each layer.</p>
<footer><button :disabled="busy" @click="step='compare'">Back</button><button :disabled="decision===model.IdentityDecision.UNSPECIFIED||busy" @click="decision===model.IdentityDecision.KEEP_SEPARATE?emit('close'):step='mapping'">Continue</button></footer>
</template>
<template v-else-if="step==='mapping'">
<h3>Choose retained identities and values</h3>
<section v-for="(draft,index) in drafts" :key="index" class="mapping" :aria-label="'Layer mapping '+(index+1)">
<h4>{{ label(draft.pair.left?.item?.kind??K.UNSPECIFIED) }}</h4>
<label>Retained identity<select v-model="draft.keep" @change="configure(draft)"><option :value="undefined" disabled>Choose explicitly</option><option value="left">{{ name(draft.pair.left?.item, draft.pair.left?.domainFields) }}</option><option value="right">{{ name(draft.pair.right?.item, draft.pair.right?.domainFields) }}</option></select></label>
<template v-if="draft.keep"><p>Retire {{ name(duplicate(draft)?.item) }}. Retain {{ name(retained(draft)?.item) }}.</p>
<button type="button" @click="duplicate(draft)?.item?.fields.forEach(field=>draft.fields[field.name]='retained')">Keep all retained field values</button>
<label v-for="field in duplicate(draft)?.item?.fields" :key="field.name">{{ field.name }}: duplicate {{ display(field) }}<select v-model="draft.fields[field.name]" :aria-label="'Resolve '+field.name+' in mapping '+(index+1)"><option :value="undefined" disabled>Choose explicitly</option><option value="retained">Keep retained: {{ display(retained(draft)?.item?.fields.find(v=>v.name===field.name)) }}</option><option value="duplicate">Use duplicate: {{ display(field) }}</option></select></label>
<label v-for="source in duplicate(draft)?.sources" :key="source">Source label: {{ source }}<input v-model="draft.sources[source]" maxlength="80" :aria-label="'Retained source name '+source"></label>
<p v-if="duplicate(draft)?.sources.length">Native bindings remain independent even with identical labels. A label change applies to all bindings with that label on this subject. Manual or unresolved legacy sources with colliding names require an explicit rename. Import configurations retain their original identity.</p>
<label v-for="relation in duplicate(draft)?.relations.filter(physical)" :key="relation.relationId"><input v-model="draft.attachments" type="checkbox" :value="relation.relationId">Confirm physical attachment {{ R[relation.kind]?.toLowerCase().replaceAll('_',' ') }}: {{ relation.from?.assetId }} → {{ relation.to?.assetId }}</label>
</template><button v-if="index" @click="drafts.splice(index,1)">Remove layer mapping</button></section>
<details v-if="decision===model.IdentityDecision.DUPLICATE_INSTALLATION"><summary>Add an installation or interface mapping</summary>
<p>Start with the hardware pair, then add the OS mapping before choosing its interfaces. Each search reads only direct children of selected layers. Unmapped installations and interfaces remain distinct.</p>
<label>Layer<select v-model="layer"><option :value="K.SOFTWARE_INSTALLATION">OS installation</option><option :value="K.INTERFACE">Interface</option></select></label><label>Find layer resource<input v-model="layerSearch" maxlength="256"></label><button :disabled="busy" @click="layers()">Find layer resources</button>
<ul><li v-for="item in layerPage.items" :key="key(item.target)">{{ name(item) }} <small>{{ layerPage.contexts.find(v=>key(v.target)===key(item.target))?.owners.map(v=>v.name).join(' / ') }}</small><button @click="layerLeft=item">Use as duplicate</button><button @click="layerRight=item">Use as retained</button></li></ul><button v-if="layerPage.next" :disabled="busy" @click="layers(layerPage.next)">Next layer results</button>
<p>Duplicate: {{ name(layerLeft) }}. Retained: {{ name(layerRight) }}.</p><button :disabled="busy||!layerLeft||!layerRight" @click="addLayer">Add layer mapping</button></details>
<p>Access grants, secrets, account control and operation targets are never transferred. Resolve responsibility, policy, source and external-binding conflicts explicitly.</p>
<footer><button :disabled="busy" @click="step='decision'">Back</button><button :disabled="busy" @click="prepare">Prepare correction preview</button></footer>
</template>
<template v-else-if="step==='preview'&&plan"><h3>Review identity correction</h3><details><summary>Exact accepted identities</summary><ul><li v-for="mapping in plan.mappings" :key="key(mapping.duplicate)">Retire {{ key(mapping.duplicate) }}; retain {{ key(mapping.retained) }}.</li></ul></details>
<section v-for="(draft,index) in drafts" :key="index" class="mapping" :aria-label="'Final layer review '+(index+1)">
<h4>Layer {{ index+1 }}</h4><p>Retire {{ name(duplicate(draft)?.item, duplicate(draft)?.domainFields) }}. Retain {{ name(retained(draft)?.item, retained(draft)?.domainFields) }}.</p>
<section v-if="retained(draft)?.item?.kind === K.PREFIX" aria-label="Prefix domain changes" class="prefix-changes">
<h5>Retained prefix before and after</h5><dl><div v-for="change in prefixChanges(draft)" :key="change.id"><dt>{{ change.label }}</dt><dd><span>Before: {{ change.beforeLabel }}</span><span>After: {{ change.afterLabel }}</span><details v-if="change.named"><summary>Exact reference</summary><span>Before: {{ change.before ? display(change.before) : 'Unknown' }}</span><span>After: {{ change.after ? display(change.after) : 'Unknown' }}</span></details></dd></div></dl>
<p v-if="duplicate(draft)?.domainFields.some(field => field.name === 'address_space')">Canonical allocation ownership moves from the retiring prefix to the retained prefix.</p>
<p v-else-if="retained(draft)?.domainFields.some(field => field.name === 'address_space')">Existing canonical allocation ownership stays with the retained prefix.</p>
<p v-else>No canonical allocation is created.</p>
</section>
<ul><li v-for="field in duplicate(draft)?.item?.fields" :key="field.name">{{ field.name }}: {{ draft.fields[field.name]==='duplicate'?'use duplicate value':'keep retained value' }}: {{ display(draft.fields[field.name]==='duplicate'?field:retained(draft)?.item?.fields.find(value=>value.name===field.name)) }}</li>
<li v-for="(after,before) in draft.sources" :key="before">Source label {{ before }} becomes {{ after }}</li>
<li>Confirmed physical attachments: {{ draft.attachments.length }}</li></ul></section>
<template v-if="plan.relations.length"><h4>Affected relationships</h4><ul><li v-for="impact in plan.relations" :key="impact.before?.relationId">{{ impact.before&&R[impact.before.kind]?.toLowerCase().replaceAll('_',' ') }}: {{ impact.before?.from?.assetId }} → {{ impact.before?.to?.assetId }}<template v-if="impact.after"> becomes {{ impact.after.from?.assetId }} → {{ impact.after.to?.assetId }}</template><template v-else> is removed as an internal attachment</template>.</li></ul></template>
<p>No grants or external operations are merged. The selected change commits atomically at revision {{ plan.revision.toString() }}.</p><footer><button :disabled="busy" @click="step='mapping'">Back</button><button :disabled="busy" @click="commit()">Accept identity correction</button></footer></template>
<template v-else-if="receipt"><h3>{{ receipt.undoUntil===0n?'Correction reversed':'Correction applied' }}</h3><p>Accepted revision {{ receipt.revision.toString() }}.</p><p v-if="receipt.undoUntil">Reversal is available until {{ new Date(Number(receipt.undoUntil)).toLocaleString() }} if no further mutations have been accepted and permissions remain current. Otherwise review a fresh correction.</p><footer><button :disabled="busy" @click="emit('close')">Close</button><button v-if="receipt.undoUntil" :disabled="busy" @click="commit(true)">Reverse this correction</button></footer></template>
<p v-if="step!=='compare'&&busy" role="status">Checking the selected correction…</p><p v-if="step!=='compare'&&error" role="alert">{{ error }}</p><button v-if="error&&!busy&&step!=='receipt'" @click="drafts=[];plan=undefined;step='compare';load()">Reload current comparison</button>
</div></Dialog></template>
<style scoped>
.identity-review { padding:1rem 1.5rem; overflow-wrap:anywhere; }
.identity-pair { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: 1rem; }
.identity-pair > section { min-width: 0; overflow-wrap: anywhere; }
.mapping {border-block-start:1px solid #cbd6d9;padding-block:1rem;overflow-wrap:anywhere;}
.prefix-changes dd span { display:block; }
@media (max-width: 700px) { .identity-pair { grid-template-columns: minmax(0, 1fr); } }
</style>
