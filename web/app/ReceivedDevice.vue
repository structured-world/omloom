<script setup lang="ts">
import {computed,onBeforeUnmount,onMounted,ref,shallowRef} from 'vue';
import AssetEditor from './AssetEditor.vue';
import PlacementSearch from './PlacementSearch.vue';
import {call,key,model,AssetKind,type Field} from './api';
import {readReceiptReferences,claimReceiptReference,clearReceiptReference,matchesReceipt,type ReceiptReference} from './receipt-reference';
const props=defineProps<{plan:model.PhysicalPlan;material:number;unit:number;working:model.WorkingContext;definitions:model.Definition[];sharedReference?:string;inactive?:boolean;recorded:(outcome:model.PhysicalReceiptOutcome,reviewed:model.PhysicalPlan,view?:model.PhysicalView)=>Promise<void>;updated:(view:model.PhysicalView,reviewed:model.PhysicalPlan,material:number,kind:'checked'|'assigned')=>Promise<void>}>();
const emit=defineEmits<{inventoryChanged:[];close:[];open:[item:import('./api').AssetRef];advance:[outcome:model.PhysicalReceiptOutcome,sharedReference?:string]}>();
const reviewedPlan=shallowRef(model.PhysicalPlan.clone(props.plan));
const explanation=ref(props.sharedReference===undefined?'':props.sharedReference),shareReference=ref(props.sharedReference!==undefined),error=ref(''),pending=ref(true),recovered=ref<model.PhysicalReceiptOutcome>();
const scope=()=>({plan:reviewedPlan.value.id!,material:props.material,unit:props.unit});
const currentRecovered=computed(()=>recovered.value?.plan?.id===reviewedPlan.value.id&&recovered.value?.material===props.material&&recovered.value?.unit===props.unit);
const unavailable=shallowRef<ReceiptReference>(),recoveryPlan=shallowRef<model.PhysicalPlan>(),recoveryRevision=ref<bigint>();
const showExisting=ref(false),existing=shallowRef<model.Item>();
async function checkReference(){
  try{
    const saved=await readReceiptReferences(scope().plan);
    const prior=saved.find(value=>matchesReceipt(value,scope()))??saved[0];
    pending.value=prior!==undefined&&matchesReceipt(prior,scope());
    if(prior)await recover(prior);
  }catch(e){error.value=String(e);}
}
onMounted(()=>void checkReference());
const inventory=model.WorkingContext.create();
const editor=ref<InstanceType<typeof AssetEditor>>(),reviewing=ref(false),recovering=ref(false);
const rejected=ref(false),inventoryOnly=ref(false),inventoryCreated=ref<import('./api').AssetRef>();
async function review(){
  if(pending.value||reviewing.value)return;
  reviewing.value=true;error.value='';
  try{
    const response=await call({oneofKind:'physical',physical:{action:{oneofKind:'load',load:{id:reviewedPlan.value.id!,revision:0n}}}},model.WorkingContext.create({...props.working,expected:undefined}));
    if(response.oneofKind!=='physical'||!response.physical.plan)throw Error('The current saved plan could not be reviewed.');
    const current=response.physical.plan,line=current.materials[props.material];
    if(![model.PhysicalPlanState.DRAFT,model.PhysicalPlanState.RESERVED,model.PhysicalPlanState.PARTIAL].includes(current.state)||!line||line.kind!==AssetKind.DEVICE||props.unit>=line.quantity||line.allocations.some(a=>a.unit===props.unit))throw Error('The current material unit no longer accepts this receipt. Input is retained.');
    if(!await editor.value?.reprepareCreation())throw Error('Current creation context could not be reviewed.');
    reviewedPlan.value=current;
    rejected.value=false;
  }catch(e){error.value=String(e);}finally{reviewing.value=false;}
}
const touched=ref(false);
function warnDraft(event:BeforeUnloadEvent){if(touched.value&&!pending.value&&!currentRecovered.value&&!inventoryCreated.value){event.preventDefault();event.returnValue='';}}
window.addEventListener('beforeunload',warnDraft);
onBeforeUnmount(()=>window.removeEventListener('beforeunload',warnDraft));
async function recover(reference?:ReceiptReference,resolveAdmission=true){
  if(recovering.value)return;
  recovering.value=true;
  error.value='';
  try{
    const saved=reference??(await readReceiptReferences(scope().plan)).find(value=>matchesReceipt(value,scope()));
    if(!saved){
      if(recovered.value)await props.recorded(recovered.value,reviewedPlan.value);
      pending.value=false;return;
    }
    const lookup={revision:BigInt(saved.revision),plan:saved.plan};
    const action:model.Physical['action']=resolveAdmission?{oneofKind:'recoverReceipt',recoverReceipt:lookup}:{oneofKind:'receiptOutcome',receiptOutcome:lookup};
    const response=await call({oneofKind:'physical',physical:{action}},model.WorkingContext.create());
    if(response.oneofKind!=='physical')throw Error('The saved device could not be checked.');
    const view=response.physical;
    if(view.receipt){
      recovered.value=view.receipt;
      await props.recorded(view.receipt,reviewedPlan.value,view.plan?view:undefined);
      await clearReceiptReference(saved);pending.value=false;
    }else if(view.receiptRecovery===model.PhysicalReceiptRecovery.NOT_COMMITTED){
      await props.updated(view,reviewedPlan.value,saved.material===undefined?props.material:saved.material,'checked');
      await clearReceiptReference(saved);pending.value=false;
      if(matchesReceipt(saved,scope())){rejected.value=true;error.value='This device was not saved. Your input is retained. Review current receipt before trying again.';}
    }else if(view.receiptRecovery===model.PhysicalReceiptRecovery.UNAVAILABLE){
      await props.updated(view,reviewedPlan.value,saved.material===undefined?props.material:saved.material,'checked');
      unavailable.value=saved;recoveryPlan.value=view.plan;recoveryRevision.value=view.revision;
      pending.value=matchesReceipt(saved,scope());
    }else throw Error('The saved device result could not be established.');
  }catch(e){error.value=String(e)+' The original outcome remains unresolved; no new device was submitted.';}finally{recovering.value=false;}
}
async function findExisting(){
  if(!unavailable.value)return;
  await recover(unavailable.value);
  if(unavailable.value)showExisting.value=true;
}
async function useExisting(){
  const saved=unavailable.value,target=existing.value?.target,plan=recoveryPlan.value;
  if(!saved||saved.material===undefined||saved.unit===undefined||!target||!plan?.id||recoveryRevision.value===undefined)return;
  recovering.value=true;error.value='';
  try{
    const working=model.WorkingContext.create({...props.working,expected:recoveryRevision.value});
    if(working.roots.length&&!working.roots.some(root=>key(root)===key(target)))working.roots.push(target);
    const response=await call({oneofKind:'physical',physical:{action:{oneofKind:'receive',receive:{plan:{id:plan.id,revision:plan.revision},material:saved.material,allocation:{unit:saved.unit,item:target,basis:'Existing device selected after checking an unavailable save result'}}}}},working);
    if(response.oneofKind!=='physical')throw Error('The existing device could not be assigned.');
    await props.updated(response.physical,plan,saved.material,'assigned');
    await clearReceiptReference(saved);unavailable.value=undefined;pending.value=false;showExisting.value=false;
    if(matchesReceipt(saved,scope()))inventoryCreated.value=target;
  }catch(e){error.value=String(e);}finally{recovering.value=false;}
}
async function submit(changes:Field[],creation:model.WorkingContext,next:boolean){
  if(!reviewedPlan.value.id||creation.expected===undefined)throw Error('Save and review this plan first.');
  if(changes.length>32)throw Error('Record at most 32 actual fields in one device creation.');
  if(new TextEncoder().encode(explanation.value).length>160||/[\u0000-\u001f\u007f-\u009f]/.test(explanation.value))throw Error('The optional reference must use at most 160 UTF-8 bytes, without control characters.');
  // Store only a non-authorizing lookup reference before the request can leave this tab.
  const saved=await claimReceiptReference({...scope(),revision:String(creation.expected)});
  if(!saved){pending.value=true;await recover(undefined,false);throw Error('An earlier save for this unit was checked. No new device was submitted.');}
  pending.value=true;
  const working=model.WorkingContext.create({...props.working,expected:creation.expected});
  try{
    const response=await call({oneofKind:'physical',physical:{action:{oneofKind:'receiveDevice',receiveDevice:{
      plan:{id:reviewedPlan.value.id,revision:reviewedPlan.value.revision},material:props.material,unit:props.unit,
      inventory:model.WorkingContext.create({...creation,expected:undefined}),changes,explanation:explanation.value.trim()?explanation.value:undefined,
    }}}},working);
    if(response.oneofKind!=='physical'||!response.physical.receipt)throw Error('The receipt response is unavailable.');
    recovered.value=response.physical.receipt;
    await props.recorded(response.physical.receipt,reviewedPlan.value);
    await clearReceiptReference(saved);pending.value=false;
    if(next)emit('advance',response.physical.receipt,shareReference.value?explanation.value:undefined);
  }catch(e){
    const meta=e instanceof Error&&'meta'in e?e.meta:undefined;
    if(meta&&typeof meta==='object'&&'omloom-mutation-outcome'in meta&&meta['omloom-mutation-outcome']==='not-committed'){
      await clearReceiptReference(saved);pending.value=false;
      rejected.value=true;
      error.value='Nothing was created by this rejected receipt. Input is retained. Review the plan or explicitly create inventory separately.';
    }else await recover(saved);
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
<p v-if="!currentRecovered">{{reviewedPlan.name}} · {{reviewedPlan.materials[material]?.name}} · Unit {{unit+1}}</p>
<p v-else>Original receipt: material {{recovered!.material+1}}, unit {{recovered!.unit+1}}.</p>
<p v-if="!currentRecovered&&!inventoryOnly">Unplaced device in permitted installation inventory. Recording confirms receipt, not installation. Planned specifications are not actual facts.</p>
<p v-if="recovered&&!currentRecovered" role="status">Earlier device saved: material {{recovered.material+1}}, unit {{recovered.unit+1}}. Receipt accepted; this form still records Unit {{unit+1}}.</p>
<template v-if="unavailable"><p role="status">Check the existing device<span v-if="unavailable.unit!==undefined"> for Unit {{unavailable.unit+1}}</span>. Its earlier save result is unavailable. Nothing will be created again automatically. Other devices can still be recorded.</p><button type="button" :disabled="recovering||unavailable.material===undefined" @click="findExisting">Find existing device</button><template v-if="showExisting"><PlacementSearch kind="material" :roots="[]" :allowed-kinds="[AssetKind.DEVICE]" @select="existing=$event"/><button type="button" :disabled="!existing||recovering" @click="useExisting">Use existing device</button></template></template>
<p v-if="error" role="alert">{{error}}</p>
<button v-if="rejected&&!pending&&!inventoryOnly&&!currentRecovered" type="button" @click="inventoryOnly=true;error=''">Create in inventory only instead</button>
<button v-if="rejected&&!pending&&!inventoryOnly&&!currentRecovered" type="button" :disabled="reviewing" @click="review">Review current receipt</button>
<template v-if="pending"><p role="status">{{currentRecovered?'Receipt accepted. Refreshing the plan.':'Checking this device save. No creation will be repeated.'}}</p><button type="button" :disabled="recovering" @click="recover()">Recover receipt result</button><button type="button" @click="emit('close')">Close, keep recovery reference</button></template>
<template v-else-if="currentRecovered"><p role="status">Receipt accepted at revision {{String(recovered!.revision)}}. Not installed. Current assignment and hold are shown in the plan.</p><button type="button" @click="openCard">Open received device</button><button type="button" @click="emit('close')">Close receiving</button></template>
<p v-if="inventoryOnly&&!inventoryCreated">This separate action creates inventory only. It does not assign or receive a unit in this plan.</p>
<template v-if="inventoryCreated"><p role="status">{{inventoryOnly?'Device created in inventory only. The plan was not changed.':'Existing device assigned. No new device was created.'}}</p><button type="button" @click="openCard">Open inventory device</button><button type="button" @click="emit('close')">Close receiving</button></template>
<div v-show="!pending&&!currentRecovered&&!inventoryCreated">
<label v-if="!inventoryOnly">Optional delivery reference or explanation<input v-model="explanation" maxlength="160"/></label>
<label v-if="!inventoryOnly&&explanation.trim()"><input v-model="shareReference" type="checkbox"/>Use this reference for the next received device</label>
<AssetEditor ref="editor" inline :inactive="inactive||reviewing||pending" :definitions="definitions" :initial-kind="AssetKind.DEVICE" :working="inventory" :view="model.View.HARDWARE" :receipt="inventoryOnly?undefined:{name:reviewedPlan.materials[material]!.name,submit}" @saved="inventorySaved" @close="emit('close')"/>
</div>
</section></template>
