<script setup lang="ts">
import { ref, computed, onBeforeUnmount } from 'vue';
import Dialog from './Dialog.vue';
import AddressDelta from './AddressDelta.vue';
import { readHostReport } from './host-report';
import { call, model, AssetKind, label, display, key, type AssetRef } from './api';
import { softwareFields } from './software-fields';
const props=defineProps<{working:model.WorkingContext;host?:model.Item}>();
const emit=defineEmits<{close:[];saved:[]}>();
const report=ref<model.HostReport>();const source=ref('');const error=ref('');
const selected=ref<string[]>([]);const matches=ref(new Map<string,{target:AssetRef;name:string}>());
const networkParts=ref<model.HostNetworkPart[]>([]);
const hostKind=ref<AssetKind>();
const sources=ref<model.HostSource[]>([]);const sourceRef=ref('');const confirmed=ref(false);
const pending=ref(false);const applyWorking=ref<model.WorkingContext>();
const available=computed(()=>new Set(sources.value.find(v=>v.reference===sourceRef.value)?.matches.map(v=>v.key)??[]));
function chooseSource(){
  const choice=sources.value.find(v=>v.reference===sourceRef.value);
  selected.value=choice?.matches.map(v=>v.key)??[];
  matches.value=new Map(choice?.matches.flatMap(v=>v.target?[[v.key,{target:v.target,name:`Existing ${v.target.assetId.slice(-8)}`}]]:[])??[]);
  preview.value=undefined;confirmed.value=false;
}
function back(){if(pending.value||busy.value)return;if(preview.value){preview.value=undefined;return;}emit('close');}
const fieldLabels=new Map(softwareFields().map(v=>[v.id,v.name]));
const unclassified=(item:model.HostFinding)=>item.kind===AssetKind.UNSPECIFIED&&item.key==='hardware';
const preview=ref<model.Preview>();const revision=ref<bigint>();const busy=ref(false);
const matching=ref('');const search=ref('');const choices=ref<model.Item[]>([]);const next=ref<string>();
let matchGeneration=0;
let fileGeneration=0;
onBeforeUnmount(()=>{fileGeneration++;matchGeneration++;});
async function file(event:Event){
  const generation=++fileGeneration;
  matchGeneration++;matching.value='';choices.value=[];next.value=undefined;revision.value=undefined;
  error.value='';preview.value=undefined;selected.value=[];networkParts.value=[];matches.value=new Map();report.value=undefined;
  hostKind.value=undefined;
  sources.value=[];sourceRef.value='';confirmed.value=false;
  try {
    const input=(event.target as HTMLInputElement).files?.[0];if(!input)return;
    if(input.size>262144)throw Error('The report exceeds the supported 256 KiB import limit.');
    const bytes=new Uint8Array(await input.arrayBuffer());
    if(generation!==fileGeneration)return;
    const inputReport=readHostReport(bytes);
    report.value=inputReport;
    if(props.host?.target){
      const response=await call({oneofKind:'hostSources',hostSources:model.HostSources.create({host:props.host.target,report:inputReport})},props.working);
      if(generation!==fileGeneration)return;
      if(response.oneofKind!=='hostSourceOptions')throw Error('Existing sources are unavailable.');
      sources.value=response.hostSourceOptions.sources;
      if(sources.value.length===1){sourceRef.value=sources.value[0]!.reference;chooseSource();}
    }
  }catch(e){if(generation===fileGeneration)error.value=String(e);}
}
async function prepare(apply:boolean){
  error.value='';busy.value=true;
  const retrying=pending.value;
  try {
    if(!apply){
      const response=await call({oneofKind:'prepare',prepare:model.Prepare.create()},model.WorkingContext.create({...props.working,expected:undefined}));
      if(response.oneofKind!=='actions')throw Error('Current import scope is unavailable.');revision.value=response.actions.revision;
    }
    if(props.host&&!confirmed.value)throw Error('Confirm that the package describes this host and its selected installation.');
    if(apply){
      if(!applyWorking.value)applyWorking.value=model.WorkingContext.create({...props.working,expected:preview.value?.revision});
      pending.value=true;
    }
    const response=await call({oneofKind:'hostImport',hostImport:model.HostImport.create(apply?{preview:preview.value?.id}:{report:report.value,source:props.host?'':source.value,refresh:props.host?{host:props.host.target,sourceRef:sourceRef.value}:undefined,selection:selected.value.map(key=>({key,target:matches.value.get(key)?.target,kind:key==='hardware'&&!matches.value.has(key)&&report.value?.findings.some(v=>v.key===key&&unclassified(v))?hostKind.value:undefined})),networkParts:networkParts.value})},apply?applyWorking.value:model.WorkingContext.create({...props.working,expected:revision.value}));
    if(apply){if(response.oneofKind!=='mutation')throw Error('Import did not commit.');emit('saved');}
    else {if(response.oneofKind==='mutation'){emit('saved');return;}if(response.oneofKind!=='preview')throw Error('Import preview unavailable.');preview.value=response.preview;applyWorking.value=undefined;}
  }catch(e){
    const meta=e instanceof Error&&'meta' in e?e.meta:undefined;
    if(apply&&!retrying&&meta&&typeof meta==='object'&&'omloom-mutation-outcome' in meta&&meta['omloom-mutation-outcome']==='not-committed'){
      pending.value=false;preview.value=undefined;applyWorking.value=undefined;
      error.value=`${String(e)} Your selections are retained. Prepare a new preview.`;
    }else error.value=pending.value?`${String(e)} Outcome unknown. Retry the same acceptance before changing this draft.`:String(e);
  }finally{busy.value=false;}
}
async function find(candidate:string,more=false){
  const generation=++matchGeneration;
  matching.value=candidate;error.value='';
  try {
    const item=report.value?.findings.find(v=>v.key===candidate);if(!item)return;
    const response=await call({oneofKind:'query',query:model.Query.create({kinds:unclassified(item)?[AssetKind.DEVICE,AssetKind.VIRTUAL_MACHINE]:[item.kind],search:search.value,roots:props.working.roots,focus:props.working.focus,columns:['name'],limit:25,cursor:more?next.value:undefined})});
    if(generation!==matchGeneration)return;
    if(response.oneofKind!=='page')throw Error('Matching resources are unavailable.');choices.value=response.page.items;next.value=response.page.next;
  }catch(e){if(generation===matchGeneration)error.value=String(e);}
}
function match(candidate:string,choice:model.Item){if(!choice.target)return;matchGeneration++;matches.value.set(candidate,{target:choice.target,name:display(choice.fields.find(v=>v.name==='name'))});matching.value='';preview.value=undefined;}
async function matchScope(candidate:string){
  const generation=++matchGeneration;
  let target=props.working.focus;
  if(!target&&props.working.roots.length===1)target=props.working.roots[0];
  if(!target)return;
  error.value='';
  try{
    const response=await call({oneofKind:'read',read:model.Read.create({target})},props.working);
    if(generation!==matchGeneration)return;
    if(response.oneofKind!=='detail'||!response.detail.item||![AssetKind.DEVICE,AssetKind.VIRTUAL_MACHINE].includes(response.detail.item.kind))throw Error('Select a device or VM scope before matching the execution host.');
    match(candidate,response.detail.item);
  }catch(e){if(generation===matchGeneration)error.value=String(e);}
}
async function identityMatches(item:model.HostFinding){
  const generation=++matchGeneration;matching.value=item.key;choices.value=[];next.value=undefined;error.value='';
  try {
    const kind=unclassified(item)?hostKind.value:item.kind;
    if(!kind)throw Error('Choose the hardware subject type before comparing its claims.');
    const response=await call({oneofKind:'findIdentityCandidates',findIdentityCandidates:model.FindIdentityCandidates.create({claims:item.identity,kind})},props.working);
    if(generation!==matchGeneration)return;
    if(response.oneofKind!=='identityCandidates')throw Error('Identity candidates are unavailable.');choices.value=response.identityCandidates.items;
    if(!choices.value.length)error.value='No comparable candidates in this selection. Missing evidence does not establish a new identity.';
  }catch(e){if(generation===matchGeneration)error.value=String(e);}
}
</script>
<template><Dialog :title="host?'Refresh host observations':'Import host observations'" :dismissible="!busy&&!pending" @close="back"><form @submit.prevent="!pending&&prepare(false)">
<p v-if="host">Update existing observations for {{ display(host.fields.find(v=>v.name==='name')) }} ({{ host.target?.assetId.slice(-8) }}). Intended fields and other sources stay unchanged. New findings require separate adoption.</p>
<p v-else>Review current facts from an explicitly collected file. Nothing is adopted until you accept the preview. Select required parents with their components.</p>
<fieldset v-show="!preview" :disabled="busy||pending"><legend>Choose observations</legend>
<label>Host report file<input type="file" accept=".omloom,.bin" @change="file"></label>
<template v-if="host&&report"><label>Existing source<select v-model="sourceRef" required @change="chooseSource"><option value="">Choose a permitted source</option><option v-for="choice in sources" :key="choice.reference" :value="choice.reference">{{ choice.label }}</option></select></label><p v-if="!sources.length">No compatible permitted source is available. Use separate adoption to establish a source mapping.</p><label><input v-model="confirmed" type="checkbox">This package describes this host and the selected OS installation</label></template>
<template v-if="!host"><label>Source name<input v-model="source" required maxlength="80" @input="preview=undefined"></label>
<p class="hint">Reuse the exact source name for later imports from this configuration. A different name selects a separate source and may propose new resources. It does not rename the existing source.</p></template>
<section v-if="report"><h3>{{ report.platform }} collection</h3>
<p>Observed {{ new Date(Number(report.observedAt)).toISOString() }}. This is the source collection time, not current freshness.</p>
<ul><li v-for="coverage in report.coverage" :key="coverage.module"><strong>{{ coverage.module }}: {{ coverage.status }}</strong> {{ coverage.detail }}</li></ul>
<p>Names, addresses and process IDs do not establish permanent identity or human ownership. Match an existing resource only after confirming it is the same incarnation.</p>
<details v-if="report.binds.some(v=>!v.owner)"><summary>Unattributed kernel binds (not adopted)</summary><ul><li v-for="(bind,index) in report.binds.filter(v=>!v.owner)" :key="index">{{ bind.value?.protocol }} {{ bind.value?.family }} {{ bind.value?.bind }}:{{ bind.value?.port }}. Owning service unknown.</li></ul></details>
<div v-for="item in report.findings" :key="item.key" class="preview">
<label><input v-model="selected" type="checkbox" :disabled="!!host&&!available.has(item.key)" :value="item.key" @change="preview=undefined">{{ unclassified(item)?'Execution host':label(item.kind) }}: {{ display(item.fields.find(v=>v.name==='name')) }}</label>
<p v-if="host&&!available.has(item.key)" class="hint">Excluded: no permitted existing match in this source. Not adopted by refresh.</p>
<template v-if="!host&&unclassified(item)&&!matches.has(item.key)"><label>New execution host type<select v-model="hostKind" aria-label="New execution host type" @change="preview=undefined"><option :value="undefined">Reuse a reviewed mapping, or choose for new adoption</option><option :value="AssetKind.DEVICE">Physical device</option><option :value="AssetKind.VIRTUAL_MACHINE">Virtual machine</option></select></label><p>The OS does not prove a physical or virtual host. Existing matches retain their type, accepted name and fields.</p></template>
<details><summary>Proposed fields ({{ item.fields.length }})</summary><dl><div v-for="field in item.fields" :key="field.name"><dt v-if="fieldLabels.has(field.name)">{{ fieldLabels.get(field.name) }}</dt><dt v-else>Source field: {{ field.name }}</dt><dd>{{ display(field) }}</dd></div></dl></details>
<p v-if="item.parent" class="hint">Inside {{ display(report.findings.find(v=>v.key===item.parent)?.fields.find(v=>v.name==='name')) }}</p>
<p v-if="item.addresses.length">{{ item.addresses.map(v=>v.address).join(', ') }}</p>
<ul v-if="report.binds.some(v=>v.owner===item.key)"><li v-for="(bind,index) in report.binds.filter(v=>v.owner===item.key)" :key="index">{{ bind.value?.protocol }} {{ bind.value?.family }} {{ bind.value?.bind }}:{{ bind.value?.port }}</li></ul>
<p v-if="matches.has(item.key)">Existing match: {{ matches.get(item.key)?.name }}</p><p v-else-if="!host">New adoption or previously reviewed source mapping</p>
<template v-if="!host"><button type="button" @click="search=''; find(item.key)">Match existing resource</button>
<button v-if="item.identity.length" type="button" @click="identityMatches(item)">Find candidates by reported identifiers</button>
<p v-if="matching===item.key&&item.identity.length">Candidates share this subject layer and at least one reported identifier. Cloned or movable identifiers are not proof. Choose a match only after review.</p>
<button v-if="unclassified(item)&&(working.focus||working.roots.length===1)" type="button" @click="matchScope(item.key)">Use current device or VM scope</button>
<button v-if="matches.has(item.key)" type="button" @click="matches.delete(item.key); preview=undefined">Clear match</button>
</template>
<section v-if="matching===item.key"><label>Find existing resource<input v-model="search" @input="find(item.key)"></label>
<button v-for="choice in choices" :key="key(choice.target)" type="button" @click="match(item.key,choice)">{{ display(choice.fields.find(v=>v.name==='name')) }}</button>
<button v-if="next" type="button" @click="find(item.key,true)">Next matches</button></section>
</div>
<fieldset v-if="report.network"><legend>Optional current network observations</legend>
<p>Select the OS installation above before importing these sections. Nothing is selected by default.</p>
<label><input v-model="networkParts" type="checkbox" :value="model.HostNetworkPart.SOCKETS" @change="preview=undefined">Binds of selected services</label>
<label><input v-model="networkParts" type="checkbox" :value="model.HostNetworkPart.ROUTES" @change="preview=undefined">Local routing table ({{ report.network.routes.length }} entries)</label>
<label><input v-model="networkParts" type="checkbox" :value="model.HostNetworkPart.APPLICATION_FIREWALL" @change="preview=undefined">Application firewall settings and rules ({{ report.network.rules.length }} entries)</label>
<details><summary>Review network coverage and configuration</summary>
<p>{{ report.network.socketCoverage }}</p><p>{{ report.network.routeCoverage }}</p><p>{{ report.network.applicationCoverage }}</p><p>{{ report.network.packetFilterCoverage }}</p>
<ul><li v-for="(route,index) in report.network.routes" :key="`route-${index}`">{{ route.family }} {{ route.destination }} via {{ route.gateway }} {{ route.interface }} {{ route.flags }}</li>
<li v-for="rule in report.network.rules" :key="rule.subject">{{ rule.subject }}: {{ rule.action }}</li></ul></details>
</fieldset></section></fieldset>
<section v-if="preview"><h3>Review {{ preview.rows.length }} selected resources</h3><p v-if="host">Source: {{ sources.find(v=>v.reference===sourceRef)?.label }}. Only selected current observations change.</p><ol><li v-for="row in preview.rows" :key="row.row">{{ row.diagnostic }}<span v-if="row.change?.changes.length">: {{ display(row.change.changes.find(v=>v.name==='name')) }}</span> <a v-if="row.change?.target" :href="`#/?asset=${encodeURIComponent(row.change.target.assetId)}&incarnation=${encodeURIComponent(row.change.target.incarnation)}`" target="_blank" rel="noopener">Inspect existing resource</a><dl v-if="host"><template v-for="field in row.change?.changes" :key="field.name"><dt>{{ fieldLabels.get(field.name)??field.name.replaceAll('_',' ') }}</dt><dd><span v-if="row.previousFields.some(v=>v.name===field.name)">Previous: {{ display(row.previousFields.find(v=>v.name===field.name)) }}. </span><span v-else>No previous claim from this source. </span>Observed: {{ display(field) }}</dd></template></dl><AddressDelta v-if="row.addresses" :value="row.addresses" /></li></ol></section>
<p v-if="error" role="alert">{{ error }}</p><footer><button type="button" :disabled="busy||pending" @click="back">{{preview?'Back':'Cancel'}}</button><button v-if="!preview" :disabled="busy||pending||!selected.length||!!host&&(!sourceRef||!confirmed)">Prepare host preview</button><button v-if="preview?.id" type="button" class="primary" :disabled="busy" @click="prepare(true)">{{pending?'Retry same acceptance':'Accept selected observations'}}</button></footer>
</form></Dialog></template>
