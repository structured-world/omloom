<script setup lang="ts">
import {onBeforeUnmount,ref} from 'vue';
import AssetEditor from './AssetEditor.vue';
import {call,model,AssetKind,type Field} from './api';
const props=defineProps<{plan:model.PhysicalPlan;material:number;unit:number;working:model.WorkingContext;definitions:model.Definition[];sharedReference?:string;inactive?:boolean;recorded:(outcome:model.PhysicalReceiptOutcome)=>Promise<void>}>();
const emit=defineEmits<{inventoryChanged:[];close:[];open:[item:import('./api').AssetRef];advance:[outcome:model.PhysicalReceiptOutcome,sharedReference?:string]}>();
const reviewedPlan=model.PhysicalPlan.clone(props.plan);
const explanation=ref(props.sharedReference===undefined?'':props.sharedReference),shareReference=ref(props.sharedReference!==undefined),error=ref(''),pending=ref(false),recovered=ref<model.PhysicalReceiptOutcome>();
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
const editor=ref<InstanceType<typeof AssetEditor>>(),reviewing=ref(false);
const rejected=ref(false),inventoryOnly=ref(false),inventoryCreated=ref<import('./api').AssetRef>();
async function review(){
  if(pending.value||reviewing.value)return;
  reviewing.value=true;error.value='';
  try{
    if(!await editor.value?.reprepareCreation())throw Error('Current creation context could not be reviewed.');
    rejected.value=false;
  }catch(e){error.value=String(e);}finally{reviewing.value=false;}
}
const touched=ref(false);
function warnDraft(event:BeforeUnloadEvent){if(touched.value&&!pending.value&&!recovered.value&&!inventoryCreated.value){event.preventDefault();event.returnValue='';}}
window.addEventListener('beforeunload',warnDraft);
onBeforeUnmount(()=>window.removeEventListener('beforeunload',warnDraft));
async function recover(){
  error.value='';
  try{
    const saved=reference();if(!saved)throw Error('No saved receipt reference is available.');
    const response=await call({oneofKind:'physical',physical:{action:{oneofKind:'receiptOutcome',receiptOutcome:{revision:BigInt(saved.revision),plan:saved.plan}}}},model.WorkingContext.create());
    if(response.oneofKind!=='physical'||!response.physical.receipt)throw Error('The original receipt result is unavailable.');
    recovered.value=response.physical.receipt;
    await props.recorded(response.physical.receipt);
    localStorage.removeItem(storageKey);pending.value=false;
  }catch(e){error.value=String(e)+' The original outcome remains unresolved; no new device was submitted.';}
}
async function submit(changes:Field[],creation:model.WorkingContext,next:boolean){
  if(reference())throw Error('Resolve the earlier receipt before creating another device.');
  if(!reviewedPlan.id||creation.expected===undefined)throw Error('Save and review this plan first.');
  if(changes.length>32)throw Error('Record at most 32 actual fields in one device creation.');
  if(new TextEncoder().encode(explanation.value).length>160||/[\u0000-\u001f\u007f-\u009f]/.test(explanation.value))throw Error('The optional reference must use at most 160 UTF-8 bytes, without control characters.');
  // Store only a non-authorizing lookup reference before the request can leave this tab.
  localStorage.setItem(storageKey,JSON.stringify({plan:reviewedPlan.id,revision:String(creation.expected)}));
  pending.value=true;
  const working=model.WorkingContext.create({...props.working,expected:creation.expected});
  try{
    const response=await call({oneofKind:'physical',physical:{action:{oneofKind:'receiveDevice',receiveDevice:{
      plan:{id:reviewedPlan.id,revision:reviewedPlan.revision},material:props.material,unit:props.unit,
      inventory:model.WorkingContext.create({...creation,expected:undefined}),changes,explanation:explanation.value.trim()?explanation.value:undefined,
    }}}},working);
    if(response.oneofKind!=='physical'||!response.physical.receipt)throw Error('The receipt response is unavailable.');
    recovered.value=response.physical.receipt;
    await props.recorded(response.physical.receipt);
    localStorage.removeItem(storageKey);pending.value=false;
    if(next)emit('advance',response.physical.receipt,shareReference.value?explanation.value:undefined);
  }catch(e){
    const meta=e instanceof Error&&'meta'in e?e.meta:undefined;
    if(meta&&typeof meta==='object'&&'omloom-mutation-outcome'in meta&&meta['omloom-mutation-outcome']==='not-committed'){
      localStorage.removeItem(storageKey);pending.value=false;
      rejected.value=true;
      error.value='Nothing was created by this rejected receipt. Input is retained. Review the plan or explicitly create inventory separately.';
    }
    throw e;
  }
}
function openCard(){
  const item=recovered.value?.item??inventoryCreated.value;if(!item)return;
  emit('open',item);
}
function inventorySaved(target:import('./api').AssetRef|undefined){if(!target)return;inventoryCreated.value=target;emit('inventoryChanged');}
</script>
<template><section aria-label="Create received device" class="received-device" @input="touched=true" @change="touched=true">
<h4>{{inventoryOnly?'Create in inventory only':'Create received device'}}</h4>
<p v-if="!recovered">{{plan.name}} · {{plan.materials[material]?.name}} · Unit {{unit+1}}</p>
<p v-else>Original receipt: material {{recovered.material+1}}, unit {{recovered.unit+1}}.</p>
<p v-if="!recovered&&!inventoryOnly">Unplaced device in permitted installation inventory. Recording confirms receipt, not installation. Planned specifications are not actual facts.</p>
<p v-if="error" role="alert">{{error}}</p>
<button v-if="rejected&&!pending&&!inventoryOnly&&!recovered" type="button" @click="inventoryOnly=true;error=''">Create in inventory only instead</button>
<button v-if="rejected&&!pending&&!inventoryOnly&&!recovered" type="button" :disabled="reviewing" @click="review">Review current receipt</button>
<template v-if="pending"><p role="status">{{recovered?'Receipt accepted. Refreshing the plan.':'An earlier receipt needs its original result. No creation will be repeated.'}}</p><button type="button" @click="recover">Recover receipt result</button><button type="button" @click="emit('close')">Close, keep recovery reference</button></template>
<template v-else-if="recovered"><p role="status">Receipt accepted at revision {{String(recovered.revision)}}. Not installed. Current assignment and hold are shown in the plan.</p><button type="button" @click="openCard">Open received device</button><button type="button" @click="emit('close')">Close receiving</button></template>
<p v-if="inventoryOnly&&!inventoryCreated">This separate action creates inventory only. It does not assign or receive a unit in this plan.</p>
<template v-if="inventoryCreated"><p role="status">Device created in inventory only. The plan was not changed.</p><button type="button" @click="openCard">Open inventory device</button><button type="button" @click="emit('close')">Close receiving</button></template>
<div v-show="!pending&&!recovered&&!inventoryCreated">
<label v-if="!inventoryOnly">Optional delivery reference or explanation<input v-model="explanation" maxlength="160"/></label>
<label v-if="!inventoryOnly&&explanation.trim()"><input v-model="shareReference" type="checkbox"/>Use this reference for the next received device</label>
<AssetEditor ref="editor" inline :inactive="inactive" :definitions="definitions" :initial-kind="AssetKind.DEVICE" :working="inventory" :view="model.View.HARDWARE" :receipt="inventoryOnly?undefined:{name:plan.materials[material]!.name,submit}" @saved="inventorySaved" @close="emit('close')"/>
</div>
</section></template>
