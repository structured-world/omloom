<script setup lang="ts">
import { ref } from 'vue';
import Dialog from './Dialog.vue';
import { call, model, label, display, key, type AssetRef } from './api';
const props=defineProps<{working:model.WorkingContext}>();
const emit=defineEmits<{close:[];saved:[]}>();
const report=ref<model.HostReport>();const source=ref('');const error=ref('');
const selected=ref<string[]>([]);const matches=ref<Record<string,AssetRef>>({});
const matchNames=ref<Record<string,string>>({});
const networkParts=ref<model.HostNetworkPart[]>([]);
const preview=ref<model.Preview>();const revision=ref<bigint>();const busy=ref(false);
const matching=ref('');const search=ref('');const choices=ref<model.Item[]>([]);const next=ref<string>();
let matchGeneration=0;
async function file(event:Event){
  matchGeneration++;matching.value='';choices.value=[];next.value=undefined;revision.value=undefined;
  error.value='';preview.value=undefined;selected.value=[];networkParts.value=[];matches.value={};matchNames.value={};report.value=undefined;
  try {
    const input=(event.target as HTMLInputElement).files?.[0];if(!input)return;
    if(input.size>262144)throw Error('The report exceeds the supported 256 KiB import limit.');
    const request=model.Request.fromBinary(new Uint8Array(await input.arrayBuffer()));
    if(request.command.oneofKind!=='hostImport'||!request.command.hostImport.report)throw Error('Choose a native Omloom host report.');
    const value=request.command.hostImport.report;
    if(value.version!==1||value.findings.length>128||value.observedAt<=0n||value.observedAt>8640000000000000n)throw Error('Unsupported report version, finding count or observation time.');
    report.value=value;
  }catch(e){error.value=String(e);}
}
async function prepare(apply:boolean){
  error.value='';busy.value=true;
  try {
    if(!apply){
      const response=await call({oneofKind:'prepare',prepare:model.Prepare.create()},model.WorkingContext.create({...props.working,expected:undefined}));
      if(response.oneofKind!=='actions')throw Error('Current import scope is unavailable.');revision.value=response.actions.revision;
    }
    const response=await call({oneofKind:'hostImport',hostImport:model.HostImport.create(apply?{preview:preview.value?.id}:{report:report.value,source:source.value,selection:selected.value.map(key=>({key,target:matches.value[key]})),networkParts:networkParts.value})},model.WorkingContext.create({...props.working,expected:apply?preview.value?.revision:revision.value}));
    if(apply){if(response.oneofKind!=='mutation')throw Error('Import did not commit.');emit('saved');}
    else {if(response.oneofKind==='mutation'){emit('saved');return;}if(response.oneofKind!=='preview')throw Error('Import preview unavailable.');preview.value=response.preview;}
  }catch(e){error.value=String(e);}finally{busy.value=false;}
}
async function find(candidate:string,more=false){
  const generation=++matchGeneration;
  matching.value=candidate;error.value='';
  try {
    const item=report.value?.findings.find(v=>v.key===candidate);if(!item)return;
    const response=await call({oneofKind:'query',query:model.Query.create({kinds:[item.kind],search:search.value,roots:props.working.roots,focus:props.working.focus,columns:['name'],limit:25,cursor:more?next.value:undefined})});
    if(generation!==matchGeneration)return;
    if(response.oneofKind!=='page')throw Error('Matching resources are unavailable.');choices.value=response.page.items;next.value=response.page.next;
  }catch(e){if(generation===matchGeneration)error.value=String(e);}
}
function match(candidate:string,choice:model.Item){if(!choice.target)return;matchGeneration++;matches.value[candidate]=choice.target;matchNames.value[candidate]=display(choice.fields.find(v=>v.name==='name'));matching.value='';preview.value=undefined;}
</script>
<template><Dialog title="Import host observations" @close="emit('close')"><form @submit.prevent="prepare(false)">
<p>Review current facts from an explicitly collected file. Nothing is adopted until you accept the preview. Select required parents with their components.</p>
<fieldset :disabled="busy"><legend>Choose observations</legend>
<label>Host report file<input type="file" accept=".omloom,.bin" @change="file"></label>
<label>Source name<input v-model="source" required maxlength="80" @input="preview=undefined"></label>
<section v-if="report"><h3>{{ report.platform }} collection</h3>
<p>Observed {{ new Date(Number(report.observedAt)).toISOString() }}. This is the source collection time, not current freshness.</p>
<ul><li v-for="coverage in report.coverage" :key="coverage.module"><strong>{{ coverage.module }}: {{ coverage.status }}</strong> {{ coverage.detail }}</li></ul>
<p>Names, addresses and process IDs do not establish permanent identity or human ownership. Match an existing resource only after confirming it is the same incarnation.</p>
<details v-if="report.binds.some(v=>!v.owner)"><summary>Unattributed kernel binds (not adopted)</summary><ul><li v-for="(bind,index) in report.binds.filter(v=>!v.owner)" :key="index">{{ bind.value?.protocol }} {{ bind.value?.family }} {{ bind.value?.bind }}:{{ bind.value?.port }}. Owning service unknown.</li></ul></details>
<div v-for="item in report.findings" :key="item.key" class="preview">
<label><input v-model="selected" type="checkbox" :value="item.key" @change="preview=undefined">{{ label(item.kind) }}: {{ display(item.fields.find(v=>v.name==='name')) }}</label>
<p v-if="item.parent" class="hint">Inside {{ display(report.findings.find(v=>v.key===item.parent)?.fields.find(v=>v.name==='name')) }}</p>
<p v-if="item.addresses.length">{{ item.addresses.map(v=>v.address).join(', ') }}</p>
<ul v-if="report.binds.some(v=>v.owner===item.key)"><li v-for="(bind,index) in report.binds.filter(v=>v.owner===item.key)" :key="index">{{ bind.value?.protocol }} {{ bind.value?.family }} {{ bind.value?.bind }}:{{ bind.value?.port }}</li></ul>
<p v-if="matches[item.key]">Existing match: {{ matchNames[item.key] }}</p><p v-else>New adoption or previously reviewed source mapping</p>
<button type="button" @click="search=''; find(item.key)">Match existing resource</button>
<button v-if="matches[item.key]" type="button" @click="delete matches[item.key]; delete matchNames[item.key]; preview=undefined">Clear match</button>
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
