<script setup lang="ts">
import { computed, ref, watch } from 'vue';
import { call, input, key, model, AssetKind as K, type AssetRef } from './api';
import PlacementSearch from './PlacementSearch.vue';
import PlanStepControls from './PlanStepControls.vue';
import { millimetres,upperMicrometres } from './physical-units';
const props = defineProps<{target:AssetRef;kind:number;working:model.WorkingContext;inactive?:boolean}>();
const emit=defineEmits<{changed:[]}>();
const opened=ref(false),busy=ref(false),error=ref(''),notice=ref('');
const view=ref(model.PhysicalView.create());
const plans=ref<model.PhysicalPlanSummary[]>([]),next=ref<string>();
const plan=ref<model.PhysicalPlan>(),name=ref(''),deadline=ref(''),discrepancy=ref('');
const assessment=ref<{state:number;reason:number;revision:bigint}>();
watch([plan,name],()=>{assessment.value=undefined;},{deep:true});
const assessmentText=computed(()=>{
  if(!assessment.value)return '';
  if(assessment.value.state===model.RequirementState.SATISFIED)return 'Declared physical constraints satisfied. Nothing was saved or reserved.';
  if(assessment.value.state===model.RequirementState.FAILED)return 'Declared physical constraints fail. Nothing was saved or reserved.';
  return `Physical result unknown: ${model.RequirementReason[assessment.value.reason]?.toLowerCase().replaceAll('_',' ')}. Nothing was saved or reserved.`;
});
const planItems=ref<model.Item[]>([]);
const equipment=ref<model.Item>(),equipmentVolume=ref<model.PhysicalVolume>();
const mode=ref<'placement'|'replacement'>(props.kind===K.RACK?'placement':'replacement');
const predecessor=ref(''),removePart=ref(false),reseat=ref(false),moving=ref(false),source=ref<AssetRef>();
const sourceView=ref<model.PhysicalView>();
const oldOccupancy=computed(()=>sourceView.value?.occupancy??view.value.occupancy);
const actualReport=ref(false),notPerformed=ref<number[]>([]);
const unit=ref('1'),side=ref('front'),lane=ref('left'),selected=ref<number[]>([]);
const working=()=>model.WorkingContext.create({roots:props.working.roots,expected:view.value.revision});
const editable=computed(()=>plan.value===undefined||plan.value.state===model.PhysicalPlanState.DRAFT);
const reservable=computed(()=>plan.value?.state===model.PhysicalPlanState.DRAFT);
const installable=computed(()=>plan.value!==undefined&&([model.PhysicalPlanState.RESERVED,model.PhysicalPlanState.PARTIAL,model.PhysicalPlanState.CANCELLED].includes(plan.value.state)||(plan.value.state===model.PhysicalPlanState.DRAFT&&actualReport.value)));
const envelope=computed(()=>view.value.definitions[0]?.volume);
const state=(value:number)=>model.PhysicalPlanState[value]?.toLowerCase();
const itemName=(item:model.Item)=>input(item.fields.find(f=>f.name==='name'));
const label=(target:AssetRef|undefined)=>{
  const item=planItems.value.find(i=>key(i.target)===key(target))??view.value.items.find(i=>key(i.target)===key(target))??sourceView.value?.items.find(i=>key(i.target)===key(target));
  const field=item?.fields.find(f=>f.name==='name');
  return field===undefined?undefined:input(field);
};
async function invoke(action:model.Physical['action']) {
  const response=await call({oneofKind:'physical',physical:{action}},working());
  if(response.oneofKind!=='physical')throw Error('Physical planning is unavailable.');
  if((action.oneofKind==='save'||action.oneofKind==='reserve'||action.oneofKind==='complete'||action.oneofKind==='cancel')&&response.physical.revision!==view.value.revision)changed=true;
  view.value.revision=response.physical.revision;
  if(response.physical.plan)planItems.value=response.physical.items;
  return response.physical;
}
let changed=false;
async function run(action:()=>Promise<void>) {
  if(busy.value||props.inactive)return;
  changed=false;
  busy.value=true;error.value='';notice.value='';
  try{await action();}catch(e){error.value=String(e);}finally{busy.value=false;if(changed)emit('changed');}
}
async function summaries(after?:string) {
  const reply=await invoke({oneofKind:'list',list:{after,limit:25}});
  plans.value=reply.plans;next.value=reply.next;
}
async function refresh() {
  const reply=await invoke({oneofKind:'read',read:{enclosure:props.target,occupants:true}});
  view.value=reply;
}
async function open() {
  if(opened.value)return;
  await run(async()=>{await refresh();await summaries();opened.value=true;});
}
async function load(saved:model.PhysicalPlanSummary) {
  await run(async()=>{
    const reply=await invoke({oneofKind:'load',load:{id:saved.id,revision:saved.revision}});
    plan.value=reply.plan;name.value=reply.plan?.name??'';selected.value=[];
  });
}
let selection=0;
async function choose(item?:model.Item) {
  const generation=++selection;equipment.value=undefined;equipmentVolume.value=undefined;
  if(!item?.target)return;
  await run(async()=>{
    const reply=await invoke({oneofKind:'read',read:{enclosure:item.target,occupants:false}});
    if(generation!==selection)return;
    equipment.value=item;
    const fields=reply.items.find(candidate=>key(candidate.target)===key(item.target))?.fields;
    const upper=(name:string)=>{
      const field=fields?.find(field=>field.name===name);
      if(field?.value.oneofKind!=='equipment'||field.value.equipment.assertion.oneofKind!=='quantity')return undefined;
      const value=upperMicrometres(field.value.equipment.assertion.quantity);
      return value===undefined||value===0n?undefined:{startUm:0n,lengthUm:value};
    };
    const x=upper('width'),y=upper('height'),z=upper('depth');
    if(x&&y&&z)equipmentVolume.value={x,y,z};
  });
}
async function chooseSource(item?:model.Item) {
  source.value=undefined;sourceView.value=undefined;predecessor.value='';
  if(!item?.target)return;
  await run(async()=>{sourceView.value=await invoke({oneofKind:'read',read:{enclosure:item.target,occupants:true}});source.value=item.target;});
}
function addReplacement() {
  error.value='';
  try{
    const old=oldOccupancy.value.find(o=>key(o.placement?.item)===predecessor.value);
    if(moving.value&&!old)throw Error('Choose the established part and its source position.');
    const replacement=moving.value||reseat.value?old?.placement?.item:removePart.value?undefined:equipment.value?.target;
    if(!old&&!replacement)throw Error('Choose a known incoming part or a known predecessor.');
    if(reseat.value&&!old)throw Error('A reseat requires the same established part.');
    const r=model.PhysicalReplacement.create({position:props.target,previous:old?.placement?.item,replacement,sourcePosition:moving.value?source.value:undefined,previousBinding:old?.binding?.relationId,previousRevision:old?.binding?.revision});
    const steps=plan.value?.steps??[];
    if(steps.length>=64)throw Error('One plan supports at most 64 physical steps.');
    plan.value=model.PhysicalPlan.create({...plan.value,name:name.value,state:model.PhysicalPlanState.DRAFT,steps:[...steps,model.PhysicalStep.create({state:model.PhysicalStepState.PLANNED,action:{oneofKind:'replacement',replacement:r}})]});
    for(const item of [...view.value.items,...(sourceView.value?.items??[]),...(equipment.value?[equipment.value]:[])]) {
      if(!planItems.value.some(i=>key(i.target)===key(item.target)))planItems.value.push(item);
    }
  }catch(e){error.value=String(e);}
}
function add() {
  error.value='';
  try{
    const v=equipmentVolume.value,rack=envelope.value;
    if(!equipment.value?.target||!v?.x||!v.y||!v.z||!rack?.x||!rack.y||!rack.z)throw Error('Known equipment and enclosure dimensions are required.');
    if(!/^[1-9][0-9]{0,2}(\.5)?$/.test(unit.value))throw Error('Enter a rack position in whole or half rack units, starting at 1.');
    const halves=BigInt(unit.value.split('.')[0]!)*2n+(unit.value.endsWith('.5')?1n:0n);
    const placement=model.PhysicalPlacement.create({item:equipment.value.target,enclosure:props.target,volume:{
      x:{startUm:rack.x.startUm+(lane.value==='right'?BigInt(rack.x.lengthUm)-BigInt(v.x.lengthUm):0n),lengthUm:v.x.lengthUm},
      y:{startUm:rack.y.startUm+(halves-2n)*22225n,lengthUm:v.y.lengthUm},
      z:{startUm:rack.z.startUm+(side.value==='rear'?BigInt(rack.z.lengthUm)-BigInt(v.z.lengthUm):0n),lengthUm:v.z.lengthUm},
    }});
    const steps=plan.value?.steps??[];
    if(steps.length>=64)throw Error('One plan supports at most 64 physical steps.');
    if(steps.some(s=>s.action.oneofKind==='placement'&&key(s.action.placement.item)===key(placement.item)))throw Error('This equipment is already in the plan.');
    plan.value=model.PhysicalPlan.create({...plan.value,name:name.value,state:model.PhysicalPlanState.DRAFT,steps:[...steps,model.PhysicalStep.create({state:model.PhysicalStepState.PLANNED,action:{oneofKind:'placement',placement}})]});
    if(!planItems.value.some(i=>key(i.target)===key(equipment.value?.target)))planItems.value.push(equipment.value);
  }catch(e){error.value=String(e);}
}
async function save() {
  await run(async()=>{
    if(!plan.value)throw Error('Add at least one placement.');
    const reply=await invoke({oneofKind:'save',save:{plan:{...plan.value,name:name.value}}});
    plan.value=reply.plan;notice.value='Plan saved. Nothing is reserved or installed.';await summaries();
  });
}
async function preview() {
  await run(async()=>{
    if(!plan.value)throw Error('Add at least one physical transition.');
    const reply=await invoke({oneofKind:'preview',preview:{plan:{...plan.value,name:name.value}}});
    assessment.value={state:reply.state,reason:reply.reason,revision:reply.revision};
  });
}
function planKey():model.PhysicalPlanKey {
  if(!plan.value?.id)throw Error('Save this plan before changing its lifecycle.');
  return {id:plan.value.id,revision:plan.value.revision};
}
async function reserve() {
  await run(async()=>{
    const value=new Date(deadline.value).getTime();
    if(!Number.isFinite(value))throw Error('Select an explicit reservation deadline.');
    const reply=await invoke({oneofKind:'reserve',reserve:{plan:planKey(),untilMs:BigInt(value)}});
    plan.value=reply.plan;notice.value='Capacity reserved. Installation is not confirmed.';await summaries();
  });
}
async function complete() {
  await run(async()=>{
    if(!plan.value||(!selected.value.length&&!notPerformed.value.length))throw Error('Select the physical outcomes actually confirmed.');
    const reply=await invoke({oneofKind:'complete',complete:model.PhysicalCompletion.create({plan:planKey(),completedSteps:selected.value,notPerformedSteps:notPerformed.value,actualState:actualReport.value,discrepancy:discrepancy.value===''?undefined:discrepancy.value})});
    plan.value=reply.plan;selected.value=[];notPerformed.value=[];notice.value='Reported installation recorded. Live health has not been asserted.';await refresh();await summaries();
  });
}
async function recheck(){await run(async()=>{await refresh();const reply=await invoke({oneofKind:'complete',complete:model.PhysicalCompletion.create({plan:planKey()})});plan.value=reply.plan;notice.value='Postconditions rechecked. No physical work was repeated.';});}
function subject(step:model.PhysicalStep){return step.action.oneofKind==='placement'?step.action.placement.item:step.action.oneofKind==='replacement'?step.action.replacement.replacement??step.action.replacement.previous:undefined;}
function subjectKind(step:model.PhysicalStep){const target=subject(step);return planItems.value.find(i=>key(i.target)===key(target))?.kind;}
function predecessors(index:number){return plan.value?.steps.slice(0,index).map(step=>({title:label(subject(step))===undefined?'Equipment name unavailable':label(subject(step))!,hasCheck:step.control?.check!==undefined}))??[];}
function removeStep(index:number){
  if(!plan.value)return;
  if(plan.value.steps.some(s=>s.control?.after.some(d=>d.step===index))){error.value='Change dependent step links before removing this step.';return;}
  plan.value.steps.splice(index,1);
  for(const step of plan.value.steps)for(const dependency of step.control?.after??[])if(dependency.step>index)dependency.step--;
}
async function cancel() {
  await run(async()=>{const reply=await invoke({oneofKind:'cancel',cancel:planKey()});plan.value=reply.plan;notice.value='Remaining work stopped. Reported facts and unresolved reservations are preserved.';await summaries();});
}
const drawing=computed(()=>{
  const rack=envelope.value;
  if(!rack?.x||!rack.y)return;
  return {width:Number(rack.x.lengthUm)/1000,height:Number(rack.y.lengthUm)/1000,x:Number(rack.x.startUm)/1000,y:Number(rack.y.startUm)/1000};
});
function rect(p:model.PhysicalPlacement) {
  const v=p.volume;
  if(!v?.x||!v.y)return;
  return {x:Number(v.x.startUm)/1000,y:Number(v.y.startUm)/1000,width:Number(v.x.lengthUm)/1000,height:Number(v.y.lengthUm)/1000};
}
const placementRows=computed(()=>plan.value?.steps.flatMap((step,index)=>step.action.oneofKind==='placement'?[{p:step.action.placement,index,state:step.state}]:[])??[]);
const replacementRows=computed(()=>plan.value?.steps.flatMap((step,index)=>step.action.oneofKind==='replacement'?[{r:step.action.replacement,index,state:step.state}]:[])??[]);
const local=computed(()=>placementRows.value.filter(row=>key(row.p.enclosure)===key(props.target)).map(row=>row.p));
</script>
<template>
<details class="physical-planner" :inert="inactive" @toggle="($event.target as HTMLDetailsElement).open&&open()">
<summary>Plan installation from enclosing scope</summary>
<p>Choose equipment from the enclosing selection. A draft changes no actual placement. Reservations and reported installation are separate actions.</p>
<p v-if="busy||notice||assessment" role="status">{{busy?'Updating physical plan…':notice?notice:assessmentText}}</p><p v-if="error" role="alert">{{error}}</p>
<p v-if="assessment">Checked at inventory revision {{assessment.revision.toString()}}. Reservation checks the current state again; this is not authority or proof of installation.</p>
<template v-if="opened">
<section aria-label="Saved physical plans"><h3>My plans</h3>
<button type="button" :disabled="busy" @click="plan=undefined;name='';selected=[]">New installation plan</button>
<ul><li v-for="saved in plans" :key="saved.id"><button type="button" :disabled="busy" @click="load(saved)">{{saved.name}} · {{state(saved.state)}}</button></li></ul>
<button v-if="next" type="button" :disabled="busy" @click="run(()=>summaries(next))">Next saved plans</button>
</section>
<div class="plan-layout">
<section v-if="drawing" aria-label="Rack elevation">
<h3>Physical elevation</h3><p>Solid: reported installed. Dashed: proposed or reserved. Opposing sides can share U positions when depth permits.</p>
<svg role="img" aria-label="Rack physical placement" :viewBox="`${drawing.x} ${drawing.y} ${drawing.width} ${drawing.height}`">
<rect :x="drawing.x" :y="drawing.y" :width="drawing.width" :height="drawing.height" class="rack"/>
<template v-for="occupant in view.occupancy" :key="key(occupant.placement?.item)"><rect v-if="occupant.placement&&rect(occupant.placement)" v-bind="rect(occupant.placement)" class="installed"><title>Reported installation: <template v-if="label(occupant.placement.item)">{{label(occupant.placement.item)}}</template><template v-else>Equipment name unavailable</template></title></rect></template>
<template v-for="placement in local" :key="key(placement.item)"><rect v-if="rect(placement)" v-bind="rect(placement)" class="proposed"><title>Planned placement: <template v-if="label(placement.item)">{{label(placement.item)}}</template><template v-else>Equipment name unavailable</template></title></rect></template>
</svg>
</section><p v-else>Set known enclosure dimensions before drawing or reserving physical capacity.</p>
<section aria-label="Installation plan editor">
<form v-if="editable" @submit.prevent="save"><fieldset :disabled="busy">
<label>Installation plan name<input v-model="name" required maxlength="160" /></label>
<div class="placement-controls"><button v-if="kind===K.RACK" type="button" :aria-pressed="mode==='placement'" @click="mode='placement'">Plan rack placement</button><button type="button" :aria-pressed="mode==='replacement'" @click="mode='replacement'">Replace, remove or move a part</button></div>
<template v-if="mode==='placement'">
<PlacementSearch kind="equipment" :roots="working().roots" @select="choose"/>
<p v-if="equipment">Selected: {{itemName(equipment)}}. {{equipmentVolume?'Accepted upper-bound dimensions available.':'Accepted dimension bounds are unknown or unavailable.'}}</p>
<div class="placement-controls"><label>Starting U<input v-model="unit" inputmode="decimal" maxlength="5"/></label>
<label>Side<select v-model="side"><option value="front">Front</option><option value="rear">Rear</option></select></label>
<label>Horizontal position<select v-model="lane"><option value="left">Left</option><option value="right">Right</option></select></label></div>
<button type="button" :disabled="!equipmentVolume" @click="add">Add planned placement</button>
</template>
<template v-else>
<label class="outcome"><input v-model="moving" type="checkbox" @change="sourceView=undefined;source=undefined;predecessor=''"/>Move an established part from another enclosure</label>
<PlacementSearch v-if="moving" kind="enclosure" :roots="working().roots" @select="chooseSource"/>
<label>Known predecessor or moving part<select v-model="predecessor"><option value="">Predecessor unresolved, not a declaration of an empty position</option><option v-for="occupant in oldOccupancy" :key="key(occupant.placement?.item)" :value="key(occupant.placement?.item)"><template v-if="label(occupant.placement?.item)">{{label(occupant.placement?.item)}}</template><template v-else>Component name unavailable</template></option></select></label>
<template v-if="!moving">
<label class="outcome"><input v-model="removePart" type="checkbox" @change="reseat=false"/>Remove the known part without installing another</label>
<label class="outcome"><input v-model="reseat" type="checkbox" @change="removePart=false"/>Reinsert the same established part</label>
<PlacementSearch v-if="!removePart&&!reseat" :kind="kind===K.RACK?'equipment':'component'" :roots="working().roots" @select="choose"/>
</template>
<p>The position and retained logical intent survive. Cable attachments, physical evidence, credentials and grants are not transferred to the incoming part.</p>
<button type="button" @click="addReplacement">Add reviewed physical transition</button>
</template>
<button type="button" :disabled="!plan?.steps.length" @click="preview">Check physical requirements</button>
<button type="submit" :disabled="!plan?.steps.length">Save installation plan</button>
</fieldset></form>
<template v-if="plan"><h3>{{plan.name||name}} <small>{{state(plan.state)}}</small></h3>
<ol class="planned-items"><li v-for="(step,index) in plan.steps" :key="index">
<template v-if="step.action.oneofKind==='placement'">
<label v-if="installable&&![model.PhysicalStepState.RECORDED,model.PhysicalStepState.NOT_PERFORMED].includes(step.state)"><input v-model="selected" type="checkbox" :value="index" :disabled="notPerformed.includes(index)||(!actualReport&&step.readiness!==model.PlanReadiness.READY)"/>Actually installed: <span v-if="label(step.action.placement.item)">{{label(step.action.placement.item)}}</span><span v-else>Equipment name unavailable</span></label>
<span v-else-if="label(step.action.placement.item)">{{label(step.action.placement.item)}}</span><span v-else>Equipment name unavailable</span>
<small v-if="step.action.placement.volume?.y">Y {{millimetres(step.action.placement.volume.y.startUm)}} mm; height {{millimetres(step.action.placement.volume.y.lengthUm)}} mm</small>
</template>
<template v-if="step.action.oneofKind==='replacement'">
<p><template v-if="step.action.replacement.previous">Previous: <span v-if="label(step.action.replacement.previous)">{{label(step.action.replacement.previous)}}</span><span v-else>Component name unavailable</span>.</template><template v-else>Predecessor unresolved.</template> <template v-if="step.action.replacement.replacement">Incoming: <span v-if="label(step.action.replacement.replacement)">{{label(step.action.replacement.replacement)}}</span><span v-else>Component name unavailable</span>.</template><template v-else>Removal without an incoming part.</template></p>
<label v-if="installable&&![model.PhysicalStepState.RECORDED,model.PhysicalStepState.NOT_PERFORMED].includes(step.state)"><input v-model="selected" type="checkbox" :value="index" :disabled="notPerformed.includes(index)||(!actualReport&&step.readiness!==model.PlanReadiness.READY)"/>Physical transition performed</label>
</template>
<small>{{model.PhysicalStepState[step.state]?.toLowerCase().replaceAll('_',' ')}}</small>
<PlanStepControls :control="step.control" :index="index" :predecessors="predecessors(index)" :subject="subject(step)" :kind="subjectKind(step)" :working="working()" :editable="editable&&!busy" :readiness="step.readiness" :checked-state="step.checkedState" :checked-reason="step.checkedReason" @change="step.control=$event;step.readiness=model.PlanReadiness.UNSPECIFIED;step.checkedState=undefined;step.checkedReason=undefined"/>
<label v-if="installable&&![model.PhysicalStepState.RECORDED,model.PhysicalStepState.NOT_PERFORMED].includes(step.state)"><input v-model="notPerformed" type="checkbox" :value="index" :disabled="selected.includes(index)"/>Confirmed not performed</label>
<button v-if="editable" type="button" :disabled="busy" @click="removeStep(index)">Remove proposed step</button>
</li></ol>
<button v-if="plan.state===model.PhysicalPlanState.PARTIAL" type="button" :disabled="busy" @click="recheck">Recheck postconditions</button>
<label v-if="plan.state!==model.PhysicalPlanState.COMPLETED" class="outcome"><input v-model="actualReport" type="checkbox"/>Record physical work already performed outside Omloom</label>
<form v-if="reservable" @submit.prevent="reserve"><label>Reserve until<input v-model="deadline" type="datetime-local" required/></label><button :disabled="busy" type="submit">Check and reserve capacity</button></form>
<form v-if="installable" @submit.prevent="complete"><label>Installation discrepancies<textarea v-model="discrepancy" maxlength="2048"/></label><p>Record only physical outcomes actually known. Incompatibility may remain a discrepancy; this report does not verify wiring or service. Other reservations remain unresolved.</p><button :disabled="busy||(!selected.length&&!notPerformed.length)" type="submit">Record selected installation</button></form>
<button v-if="plan.state!==model.PhysicalPlanState.COMPLETED&&plan.state!==model.PhysicalPlanState.CANCELLED" :disabled="busy" type="button" @click="cancel">Stop remaining work</button>
</template>
</section></div>
</template>
</details>
</template>
<style scoped>
.physical-planner{margin-block:1rem;min-width:0}.physical-planner fieldset{border:0;padding:0;min-width:0}.physical-planner label{display:grid;gap:.35rem;margin-block:.75rem}.physical-planner button,.physical-planner input,.physical-planner select{min-height:44px;max-width:100%;box-sizing:border-box}.physical-planner textarea{width:100%;min-height:90px;box-sizing:border-box}.physical-planner ul{padding-left:1rem}.physical-planner small{display:block;overflow-wrap:anywhere}.plan-layout{display:grid;grid-template-columns:minmax(180px,1fr) minmax(260px,2fr);gap:1.5rem}.plan-layout>section{min-width:0}.plan-layout svg{display:block;width:100%;max-height:660px;background:#eef3f3}.rack{fill:#fff;stroke:#365c65;stroke-width:2;vector-effect:non-scaling-stroke}.installed{fill:#a6c6ce;stroke:#244c58;stroke-width:2;vector-effect:non-scaling-stroke}.proposed{fill:#d9e5be66;stroke:#586b28;stroke-width:2;stroke-dasharray:5 4;vector-effect:non-scaling-stroke}.placement-controls{display:grid;grid-template-columns:repeat(3,minmax(0,1fr));gap:.5rem}.planned-items li{padding-block:.5rem;overflow-wrap:anywhere}.planned-items label{display:flex;align-items:center}.physical-planner section>ul button{text-align:left}@media(max-width:700px){.plan-layout{display:flex;flex-direction:column-reverse}.plan-layout svg{max-height:420px}.placement-controls{grid-template-columns:1fr}.physical-planner button{width:100%;margin-block:.25rem}}
.outcome{display:flex!important;align-items:center;gap:.5rem}
</style>
