<script setup lang="ts">
import {ref} from 'vue';
import {useRouter} from 'vue-router';
import AssetEditor from './AssetEditor.vue';
import {call,model,AssetKind,type Field} from './api';
const props=defineProps<{plan:model.PhysicalPlan;material:number;unit:number;working:model.WorkingContext;definitions:model.Definition[]}>();
const emit=defineEmits<{recorded:[outcome:model.PhysicalReceiptOutcome,next:boolean];close:[]}>();
const router=useRouter();
const reviewedPlan=model.PhysicalPlan.clone(props.plan);
const explanation=ref(''),error=ref(''),pending=ref(false),recovered=ref<model.PhysicalReceiptOutcome>();
const storageKey='omloom.pending-device-receipt';
type Reference={plan:string;revision:string};
function reference():Reference|undefined {
  const raw=localStorage.getItem(storageKey);
  if(raw===null)return undefined;
  if(raw.length>256)throw Error('The saved receipt reference is invalid. Do not submit another receipt until it is resolved.');
  const value:unknown=JSON.parse(raw);
  if(typeof value!=='object'||value===null||!('plan'in value)||!('revision'in value)||typeof value.plan!=='string'||typeof value.revision!=='string'||!/^[0-9a-f-]{36}$/.test(value.plan)||!/^\d{1,20}$/.test(value.revision))throw Error('The saved receipt reference is invalid.');
  return {plan:value.plan,revision:value.revision};
}
try{pending.value=reference()!==undefined;}catch(e){pending.value=true;error.value=String(e);}
const inventory=model.WorkingContext.create();
async function recover(){
  error.value='';
  try{
    const saved=reference();if(!saved)throw Error('No saved receipt reference is available.');
    const response=await call({oneofKind:'physical',physical:{action:{oneofKind:'receiptOutcome',receiptOutcome:{revision:BigInt(saved.revision),plan:saved.plan}}}},model.WorkingContext.create());
    if(response.oneofKind!=='physical'||!response.physical.receipt)throw Error('The original receipt result is unavailable.');
    recovered.value=response.physical.receipt;
    localStorage.removeItem(storageKey);pending.value=false;
    emit('recorded',response.physical.receipt,false);
  }catch(e){error.value=String(e)+' The original outcome remains unresolved; no new device was submitted.';}
}
async function submit(changes:Field[],creation:model.WorkingContext,next:boolean){
  if(reference())throw Error('Resolve the earlier receipt before creating another device.');
  if(!reviewedPlan.id||creation.expected===undefined)throw Error('Save and review this plan first.');
  // Store only a non-authorizing lookup reference before the request can leave this tab.
  localStorage.setItem(storageKey,JSON.stringify({plan:reviewedPlan.id,revision:String(creation.expected)}));
  pending.value=true;
  const working=model.WorkingContext.create({...props.working,expected:creation.expected});
  try{
    const response=await call({oneofKind:'physical',physical:{action:{oneofKind:'receiveDevice',receiveDevice:{
      plan:{id:reviewedPlan.id,revision:reviewedPlan.revision},material:props.material,unit:props.unit,
      inventory:creation,changes,explanation:explanation.value.trim()?explanation.value:undefined,
    }}}},working);
    if(response.oneofKind!=='physical'||!response.physical.receipt)throw Error('The receipt response is unavailable.');
    recovered.value=response.physical.receipt;localStorage.removeItem(storageKey);pending.value=false;
    emit('recorded',response.physical.receipt,next);
  }catch(e){
    const meta=e instanceof Error&&'meta'in e?e.meta:undefined;
    if(meta&&typeof meta==='object'&&'omloom-mutation-outcome'in meta&&meta['omloom-mutation-outcome']==='not-committed'){
      localStorage.removeItem(storageKey);pending.value=false;
      error.value='Nothing was created by this rejected receipt. Input is retained. Review the plan or explicitly create inventory separately.';
    }
    throw e;
  }
}
function openCard(){
  const item=recovered.value?.item;if(!item)return;
  void router.push({path:'/',query:{asset:item.assetId,incarnation:item.incarnation}});
}
</script>
<template><section aria-label="Create received device" class="received-device">
<h4>Create received device</h4>
<p>{{plan.name}} · {{plan.materials[material]?.name}} · Unit {{unit+1}}</p>
<p>Creation context: permitted installation inventory. Physical placement: not recorded. Planned specifications are not actual facts.</p>
<p>Recording attests that this item was received. It does not install it, verify fit or complete work. Actor and recording time come from the server.</p>
<p v-if="error" role="alert">{{error}}</p>
<template v-if="pending"><p role="status">An earlier receipt needs its original result. No creation will be repeated.</p><button type="button" @click="recover">Recover receipt result</button><button type="button" @click="emit('close')">Close, keep recovery reference</button></template>
<template v-else-if="recovered"><p role="status">Receipt accepted at revision {{String(recovered.revision)}}. Current allocation and hold are shown by the reloaded plan, separately from this original result.</p><button type="button" @click="openCard">Open received device</button><button type="button" @click="emit('close')">Close receiving</button></template>
<div v-show="!pending&&!recovered">
<label>Optional delivery reference or explanation<input v-model="explanation" maxlength="480"/></label>
<AssetEditor inline :definitions="definitions" :initial-kind="AssetKind.DEVICE" :working="inventory" :view="model.View.HARDWARE" :receipt="{name:plan.materials[material]!.name,submit}" @close="emit('close')"/>
</div>
</section></template>
