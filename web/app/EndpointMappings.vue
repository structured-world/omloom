<script setup lang="ts">
import { computed,ref,watch } from 'vue';
import {call,model,AssetKind,key,input,type AssetRef} from './api';
import PlacementSearch from './PlacementSearch.vue';
const props=defineProps<{replacement:model.PhysicalReplacement;editable:boolean;items:model.Item[];remaining:number}>();
const emit=defineEmits<{change:[pairs:model.PhysicalEndpointMapping[]];selected:[item:model.Item]}>();
const old=ref<model.Item>(),incoming=ref<model.Item>();
const oldChannel=ref(''),incomingChannel=ref(''),selectionEpoch=ref(0);
const inventory=ref<model.Page>(),inventoryBusy=ref(false),inventoryError=ref('');
const panel=ref<HTMLDetailsElement>();
let generation=0;
watch(()=>key(props.replacement.previous),()=>{generation++;inventory.value=undefined;inventoryBusy.value=false;if(panel.value?.open)void loadPorts();});
async function loadPorts(cursor?:string){
  if(!props.replacement.previous||inventoryBusy.value)return;
  const turn=++generation;inventoryBusy.value=true;inventoryError.value='';
  try{const reply=await call({oneofKind:'query',query:model.Query.create({roots:[props.replacement.previous],kinds:[AssetKind.PORT],view:model.View.HARDWARE,columns:['name'],levelOnly:true,limit:25,cursor})});
    if(reply.oneofKind!=='page')throw Error('Previous ports are unavailable.');
    if(turn===generation)inventory.value=reply.page;
  }catch(e){if(turn===generation)inventoryError.value=String(e);}finally{if(turn===generation)inventoryBusy.value=false;}
}
const byPort=computed(()=>{const result=new Map<string,model.PhysicalEndpointMapping[]>();for(const pair of props.replacement.endpoints){const id=key(pair.previous);let rows=result.get(id);if(rows===undefined){rows=[];result.set(id,rows);}rows.push(pair);}return result;});
function coverage(item:model.Item){const pairs=byPort.value.get(key(item.target));if(pairs===undefined)return 'Unmapped / not reviewed';if(pairs.some(p=>p.previousChannel!==undefined))return `${pairs.length} channel decisions; other channels remain unqualified`;return model.PhysicalMappingDecision[pairs[0]!.decision]?.toLowerCase();}
const decision=ref(model.PhysicalMappingDecision.UNRESOLVED),error=ref(''),page=ref(0);
watch([()=>key(props.replacement.previous),()=>key(props.replacement.replacement)],()=>{old.value=undefined;incoming.value=undefined;page.value=0;oldChannel.value='';incomingChannel.value='';selectionEpoch.value++;});
const rows=computed(()=>props.replacement.endpoints.slice(page.value*25,(page.value+1)*25));
function name(target:AssetRef|undefined){const item=props.items.find(i=>key(i.target)===key(target));return item===undefined?'Port name unavailable':input(item.fields.find(f=>f.name==='name'));}
function select(side:'previous'|'replacement',item?:model.Item){if(side==='previous')old.value=item;else incoming.value=item;if(item)emit('selected',item);}
function apply(){
  error.value='';
  if(!old.value?.target){error.value='Choose a port belonging to the previous component.';return;}
  if(decision.value!==model.PhysicalMappingDecision.UNRESOLVED&&!incoming.value?.target){error.value='Choose the proposed incoming port before accepting or rejecting the pair.';return;}
  const pairs=[...props.replacement.endpoints];
  function channel(text:string){if(text==='')return undefined;if(!/^[1-9][0-9]{0,3}$/.test(text)||Number(text)>4096)throw Error('Use an explicit channel from 1 to 4096 or leave the entire port selected.');return Number(text);}
  let previousChannel:number|undefined,replacementChannel:number|undefined;
  try{previousChannel=channel(oldChannel.value);replacementChannel=decision.value===model.PhysicalMappingDecision.UNRESOLVED?undefined:channel(incomingChannel.value);}catch(e){error.value=String(e);return;}
  const index=pairs.findIndex(p=>key(p.previous)===key(old.value?.target)&&p.previousChannel===previousChannel);
  if(index===-1&&props.remaining===0){error.value='One plan supports at most 64 endpoint decisions.';return;}
  const destination=decision.value===model.PhysicalMappingDecision.UNRESOLVED?undefined:incoming.value?.target;
  if(pairs.some((p,i)=>i!==index&&key(p.previous)===key(old.value?.target)&&(p.previousChannel===undefined||previousChannel===undefined))){error.value='Choose either the entire previous port or its individual channels.';return;}
  if(decision.value===model.PhysicalMappingDecision.ACCEPTED&&pairs.some((p,i)=>i!==index&&p.decision===model.PhysicalMappingDecision.ACCEPTED&&key(p.replacement)===key(destination)&&(p.replacementChannel===undefined||replacementChannel===undefined||p.replacementChannel===replacementChannel))){error.value='This destination is already allocated. Breakout requires separately identified channels.';return;}
  const pair=model.PhysicalEndpointMapping.create({previous:old.value.target,replacement:destination,decision:decision.value,previousChannel,replacementChannel});
  if(index===-1)pairs.push(pair);else pairs[index]=pair;
  emit('change',pairs);old.value=undefined;incoming.value=undefined;selectionEpoch.value++;
}
</script>
<template>
<details ref="panel" class="endpoint-mappings" @toggle="($event.target as HTMLDetailsElement).open&&!inventory&&loadPorts()"><summary>Review endpoint correspondence ({{replacement.endpoints.length}})</summary>
<p>Accepting the component replacement does not accept a port pair. A correspondence retains intent only. Cable attachment, compatibility, configuration and service verification require their own steps and evidence. Ports not reviewed here remain unmapped.</p>
<section aria-label="Previous endpoint coverage"><p v-if="inventoryBusy" role="status">Loading this page of previous ports…</p><p v-if="inventoryError" role="alert">{{inventoryError}}</p><template v-if="inventory"><p>{{inventory.total.toString()}} authorized direct ports. This is not coverage of hidden hardware.</p><ul><li v-for="item in inventory.items" :key="key(item.target)"><strong>{{input(item.fields.find(f=>f.name==='name'))}}</strong> · {{coverage(item)}}</li></ul></template><button type="button" :disabled="inventoryBusy" @click="loadPorts()">First previous-port page</button><button v-if="inventory?.next" type="button" :disabled="inventoryBusy" @click="loadPorts(inventory.next)">Next previous-port page</button></section>
<fieldset v-if="editable&&replacement.previous&&replacement.replacement"><legend>One endpoint decision</legend>
<section aria-label="Previous component port"><PlacementSearch :key="selectionEpoch" kind="port" :roots="[replacement.previous]" direct @select="select('previous',$event)"/></section>
<label>Previous channel, if selected<input v-model="oldChannel" inputmode="numeric" maxlength="4" placeholder="Entire port"/></label>
<section aria-label="Incoming component port"><PlacementSearch :key="selectionEpoch" kind="port" :roots="[replacement.replacement]" direct @select="select('replacement',$event)"/></section>
<label>Incoming channel, if selected<input v-model="incomingChannel" inputmode="numeric" maxlength="4" placeholder="Entire port"/></label>
<label>Endpoint decision<select v-model="decision"><option :value="model.PhysicalMappingDecision.UNRESOLVED">Leave unresolved</option><option :value="model.PhysicalMappingDecision.ACCEPTED">Accept this correspondence</option><option :value="model.PhysicalMappingDecision.REJECTED">Reject this pair</option></select></label>
<button type="button" @click="apply">Apply endpoint decision to draft</button><p v-if="error" role="alert">{{error}}</p></fieldset>
<ul><li v-for="(pair,index) in rows" :key="key(pair.previous)+'/'+pair.previousChannel"><strong>{{name(pair.previous)}}</strong><span v-if="pair.previousChannel!==undefined"> channel {{pair.previousChannel}}</span> → {{pair.replacement?name(pair.replacement):'Unmapped'}}<span v-if="pair.replacementChannel!==undefined"> channel {{pair.replacementChannel}}</span> · {{model.PhysicalMappingDecision[pair.decision]?.toLowerCase()}}
<button v-if="editable" type="button" :aria-label="'Remove endpoint decision '+(page*25+index+1)" @click="emit('change',replacement.endpoints.filter((_,i)=>i!==page*25+index));page=0">Remove decision</button></li></ul>
<button v-if="page>0" type="button" @click="page--">Previous endpoint decisions</button><button v-if="(page+1)*25<replacement.endpoints.length" type="button" @click="page++">Next endpoint decisions</button>
</details>
</template>
<style scoped>
.endpoint-mappings{min-width:0;margin-block:1rem}.endpoint-mappings fieldset{min-width:0;margin-block:1rem}.endpoint-mappings label{display:grid;gap:.4rem}.endpoint-mappings li{overflow-wrap:anywhere}.endpoint-mappings button,.endpoint-mappings select{min-height:44px;max-width:100%}@media(max-width:700px){.endpoint-mappings button{width:100%}}
</style>
