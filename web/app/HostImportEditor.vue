<script setup lang="ts">
import { ref, onBeforeUnmount } from 'vue';
import Dialog from './Dialog.vue';
import { readHostReport } from './host-report';
import { call, model, AssetKind, label, display, key, type AssetRef } from './api';
import { softwareFields } from './software-fields';
const props=defineProps<{working:model.WorkingContext}>();
const emit=defineEmits<{close:[];saved:[]}>();
const report=ref<model.HostReport>();const source=ref('');const error=ref('');
const selected=ref<string[]>([]);const matches=ref(new Map<string,{target:AssetRef;name:string}>());
const networkParts=ref<model.HostNetworkPart[]>([]);
const hostKind=ref<AssetKind>();
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
  try {
    const input=(event.target as HTMLInputElement).files?.[0];if(!input)return;
    if(input.size>262144)throw Error('The report exceeds the supported 256 KiB import limit.');
    const bytes=new Uint8Array(await input.arrayBuffer());
    if(generation!==fileGeneration)return;
    report.value=readHostReport(bytes);
  }catch(e){if(generation===fileGeneration)error.value=String(e);}
}
async function prepare(apply:boolean){
  error.value='';busy.value=true;
  try {
    if(!apply){
      const response=await call({oneofKind:'prepare',prepare:model.Prepare.create()},model.WorkingContext.create({...props.working,expected:undefined}));
      if(response.oneofKind!=='actions')throw Error('Current import scope is unavailable.');revision.value=response.actions.revision;
    }
    const response=await call({oneofKind:'hostImport',hostImport:model.HostImport.create(apply?{preview:preview.value?.id}:{report:report.value,source:source.value,selection:selected.value.map(key=>({key,target:matches.value.get(key)?.target,kind:key==='hardware'&&!matches.value.has(key)&&report.value?.findings.some(v=>v.key===key&&unclassified(v))?hostKind.value:undefined})),networkParts:networkParts.value})},model.WorkingContext.create({...props.working,expected:apply?preview.value?.revision:revision.value}));
    if(apply){if(response.oneofKind!=='mutation')throw Error('Import did not commit.');emit('saved');}
    else {if(response.oneofKind==='mutation'){emit('saved');return;}if(response.oneofKind!=='preview')throw Error('Import preview unavailable.');preview.value=response.preview;}
  }catch(e){error.value=String(e);}finally{busy.value=false;}
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
</script>
<template><Dialog title="Import host observations" @close="emit('close')"><form @submit.prevent="prepare(false)">
<p>Review current facts from an explicitly collected file. Nothing is adopted until you accept the preview. Select required parents with their components.</p>
<fieldset :disabled="busy"><legend>Choose observations</legend>
<label>Host report file<input type="file" accept=".omloom,.bin" @change="file"></label>
<label>Source name<input v-model="source" required maxlength="80" @input="preview=undefined"></label>
<p class="hint">Reuse the exact source name for later imports from this configuration. A different name selects a separate source and may propose new resources. It does not rename the existing source.</p>
<section v-if="report"><h3>{{ report.platform }} collection</h3>
<p>Observed {{ new Date(Number(report.observedAt)).toISOString() }}. This is the source collection time, not current freshness.</p>
<ul><li v-for="coverage in report.coverage" :key="coverage.module"><strong>{{ coverage.module }}: {{ coverage.status }}</strong> {{ coverage.detail }}</li></ul>
<p>Names, addresses and process IDs do not establish permanent identity or human ownership. Match an existing resource only after confirming it is the same incarnation.</p>
<details v-if="report.binds.some(v=>!v.owner)"><summary>Unattributed kernel binds (not adopted)</summary><ul><li v-for="(bind,index) in report.binds.filter(v=>!v.owner)" :key="index">{{ bind.value?.protocol }} {{ bind.value?.family }} {{ bind.value?.bind }}:{{ bind.value?.port }}. Owning service unknown.</li></ul></details>
<div v-for="item in report.findings" :key="item.key" class="preview">
<label><input v-model="selected" type="checkbox" :value="item.key" @change="preview=undefined">{{ unclassified(item)?'Execution host':label(item.kind) }}: {{ display(item.fields.find(v=>v.name==='name')) }}</label>
<template v-if="unclassified(item)&&!matches.has(item.key)"><label>New execution host type<select v-model="hostKind" aria-label="New execution host type" @change="preview=undefined"><option :value="undefined">Reuse a reviewed mapping, or choose for new adoption</option><option :value="AssetKind.DEVICE">Physical device</option><option :value="AssetKind.VIRTUAL_MACHINE">Virtual machine</option></select></label><p>The OS does not prove a physical or virtual host. Existing matches retain their type, accepted name and fields.</p></template>
<details><summary>Proposed fields ({{ item.fields.length }})</summary><dl><div v-for="field in item.fields" :key="field.name"><dt v-if="fieldLabels.has(field.name)">{{ fieldLabels.get(field.name) }}</dt><dt v-else>Source field: {{ field.name }}</dt><dd>{{ display(field) }}</dd></div></dl></details>
<p v-if="item.parent" class="hint">Inside {{ display(report.findings.find(v=>v.key===item.parent)?.fields.find(v=>v.name==='name')) }}</p>
<p v-if="item.addresses.length">{{ item.addresses.map(v=>v.address).join(', ') }}</p>
<ul v-if="report.binds.some(v=>v.owner===item.key)"><li v-for="(bind,index) in report.binds.filter(v=>v.owner===item.key)" :key="index">{{ bind.value?.protocol }} {{ bind.value?.family }} {{ bind.value?.bind }}:{{ bind.value?.port }}</li></ul>
<p v-if="matches.has(item.key)">Existing match: {{ matches.get(item.key)?.name }}</p><p v-else>New adoption or previously reviewed source mapping</p>
<button type="button" @click="search=''; find(item.key)">Match existing resource</button>
<button v-if="unclassified(item)&&(working.focus||working.roots.length===1)" type="button" @click="matchScope(item.key)">Use current device or VM scope</button>
<button v-if="matches.has(item.key)" type="button" @click="matches.delete(item.key); preview=undefined">Clear match</button>
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
<section v-if="preview"><h3>Review {{ preview.rows.length }} selected resources</h3><ol><li v-for="row in preview.rows" :key="row.row">{{ row.diagnostic }}<span v-if="row.change?.changes.length">: {{ display(row.change.changes.find(v=>v.name==='name')) }}</span> <a v-if="row.change?.target" :href="`#/?asset=${encodeURIComponent(row.change.target.assetId)}&incarnation=${encodeURIComponent(row.change.target.incarnation)}`" target="_blank" rel="noopener">Inspect existing resource</a></li></ol></section>
<p v-if="error" role="alert">{{ error }}</p><footer><button type="button" @click="emit('close')">Cancel</button><button :disabled="busy||!selected.length">Prepare host preview</button><button v-if="preview?.id" type="button" class="primary" :disabled="busy" @click="prepare(true)">Accept selected observations</button></footer>
</form></Dialog></template>
