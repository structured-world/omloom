<script setup lang="ts">
import {ref} from 'vue';
import {call,model,key,input,enumLabel,requestMessage,type AssetRef} from './api';
import PlacementSearch from './PlacementSearch.vue';
const props=defineProps<{target:AssetRef;working:model.WorkingContext;plan?:boolean}>();
const emit=defineEmits<{configure:[query:model.EvaluateRequirements]}>();
const sources=ref<(model.Item|undefined)[]>([undefined,undefined]);
const boundaries=ref<model.Item[]>([]);
const candidate=ref<model.Item>();
const busy=ref(false),error=ref('');
const result=ref<model.RequirementEvaluation>();
function changed(){result.value=undefined;error.value='';}
function source(index:number,item?:model.Item){sources.value[index]=item;changed();}
function boundary(){if(!candidate.value?.target)return;if(boundaries.value.some(i=>i.target&&key(i.target)===key(candidate.value!.target!)))return;boundaries.value.push(candidate.value);changed();}
async function check(){
  changed();
  try{
    if(!sources.value.every(i=>i?.target))throw Error('Choose both feeds.');
    const query=model.EvaluateRequirements.create({anchor:props.target,root:0,nodes:[{expression:{oneofKind:'powerPaths',powerPaths:{sources:sources.value.map(i=>i!.target!),boundaries:boundaries.value.map(i=>i.target!)}}}]});
    if(props.plan){emit('configure',query);return;}
    busy.value=true;
    const response=await call({oneofKind:'evaluateRequirements',evaluateRequirements:query},props.working);
    if(response.oneofKind!=='requirementEvaluation')throw Error('Power path result unavailable.');
    result.value=response.requirementEvaluation;
  }catch(e){error.value=requestMessage(e);}finally{busy.value=false;}
}
</script>
<template><details class="power-paths" aria-label="Recorded power paths"><summary>{{plan?'Configure a power path postcondition':'Compare recorded power paths'}}</summary>
<p>Compare two feeds through their explicitly recorded upstream dependencies. Add terminal sources when known. Without them this can identify shared components, but cannot establish separation. A positive result needs complete source lists along both paths. This checks recorded path separation, not live power, surviving capacity or electrical safety.</p>
<fieldset :disabled="busy"><legend>Selected power paths</legend>
<section v-for="(_,i) in sources" :key="i" :aria-label="`Input feed ${i+1}`"><h4>Input feed {{i+1}}</h4><PlacementSearch kind="power" :roots="working.roots" @select="source(i,$event)"/></section>
<section aria-label="Terminal power boundaries"><h4>Terminal power boundaries</h4><PlacementSearch kind="power" :roots="working.roots" @select="candidate=$event"/><button type="button" :disabled="!candidate||boundaries.length===8" @click="boundary">Add terminal boundary</button>
<ul><li v-for="(entry,i) in boundaries" :key="key(entry.target!)">{{input(entry.fields.find(f=>f.name==='name'))}} <button type="button" @click="boundaries.splice(i,1);changed()">Remove boundary</button></li></ul></section>
<button type="button" :disabled="busy||sources.some(i=>!i?.target)" @click="check">{{plan?'Use power path postcondition':'Check power path separation'}}</button>
</fieldset><p v-if="error" role="alert">{{error}}</p>
<section v-if="result" aria-label="Power path result" aria-live="polite"><h4>{{result.state===model.RequirementState.SATISFIED?'Recorded paths are disjoint within the selected boundaries':result.state===model.RequirementState.FAILED?'Recorded paths share a dependency':'Power path separation is not established'}}</h4><p>{{enumLabel(model.RequirementReason,result.reason).replaceAll('_',' ')}} · Inventory revision {{result.revision.toString()}}</p><a v-if="result.results[0]?.target" :href="`#/?view=2&asset=${encodeURIComponent(result.results[0].target.assetId)}&incarnation=${encodeURIComponent(result.results[0].target.incarnation)}`">Inspect the shared or unqualified resource</a><p>This transient assessment grants no authority and confirms no live connectivity.</p></section>
</details></template>
<style scoped>
.power-paths{margin-block:1rem;min-width:0}.power-paths fieldset{border:0;padding:0}.power-paths section{margin-block:.75rem;min-width:0}.power-paths button{min-height:44px;max-width:100%}.power-paths li{overflow-wrap:anywhere}@media(max-width:700px){.power-paths button{width:100%}}
</style>
