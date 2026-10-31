<script setup lang="ts">
import { computed, ref } from 'vue';
import { call, input, key, model, type AssetRef } from './api';
import PlacementSearch from './PlacementSearch.vue';
import { millimetres } from './physical-units';
const props = defineProps<{target:AssetRef;working:model.WorkingContext}>();
const opened=ref(false),busy=ref(false),error=ref(''),notice=ref('');
const view=ref(model.PhysicalView.create());
const plans=ref<model.PhysicalPlanSummary[]>([]),next=ref<string>();
const plan=ref<model.PhysicalPlan>(),name=ref(''),deadline=ref(''),discrepancy=ref('');
const planItems=ref<model.Item[]>([]);
const equipment=ref<model.Item>(),definition=ref<model.PhysicalDefinition>();
const unit=ref('1'),side=ref('front'),lane=ref('left'),selected=ref<string[]>([]);
const working=()=>model.WorkingContext.create({roots:props.working.roots,expected:view.value.revision});
const editable=computed(()=>plan.value===undefined||plan.value.state===model.PhysicalPlanState.DRAFT);
const reservable=computed(()=>plan.value?.state===model.PhysicalPlanState.DRAFT);
const installable=computed(()=>plan.value!==undefined&&[model.PhysicalPlanState.RESERVED,model.PhysicalPlanState.PARTIAL].includes(plan.value.state));
const envelope=computed(()=>view.value.definitions[0]?.volume);
const state=(value:number)=>model.PhysicalPlanState[value]?.toLowerCase();
const itemName=(item:model.Item)=>input(item.fields.find(f=>f.name==='name'));
const label=(target:AssetRef|undefined)=>{
  const item=planItems.value.find(i=>key(i.target)===key(target))??view.value.items.find(i=>key(i.target)===key(target));
  const field=item?.fields.find(f=>f.name==='name');
  return field===undefined?undefined:input(field);
};
async function invoke(action:model.Physical['action']) {
  const response=await call({oneofKind:'physical',physical:{action}},working());
  if(response.oneofKind!=='physical')throw Error('Physical planning is unavailable.');
  view.value.revision=response.physical.revision;
  if(response.physical.plan)planItems.value=response.physical.items;
  return response.physical;
}
async function run(action:()=>Promise<void>) {
  if(busy.value)return;
  busy.value=true;error.value='';notice.value='';
  try{await action();}catch(e){error.value=String(e);}finally{busy.value=false;}
}
async function summaries(after?:string) {
  const reply=await invoke({oneofKind:'list',list:{after,limit:25}});
  plans.value=reply.plans;next.value=reply.next;
}
async function refresh() {
  const reply=await invoke({oneofKind:'read',read:{enclosure:props.target}});
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
  const generation=++selection;equipment.value=undefined;definition.value=undefined;
  if(!item?.target)return;
  await run(async()=>{
    const reply=await invoke({oneofKind:'read',read:{enclosure:item.target}});
    if(generation===selection){equipment.value=item;definition.value=reply.definitions[0];}
  });
}
function add() {
  error.value='';
  try{
    const v=definition.value?.volume,rack=envelope.value;
    if(!equipment.value?.target||!v?.x||!v.y||!v.z||!rack?.x||!rack.y||!rack.z)throw Error('Known equipment and enclosure dimensions are required.');
    if(!/^[1-9][0-9]{0,2}(\.5)?$/.test(unit.value))throw Error('Enter a rack position in whole or half rack units, starting at 1.');
    const halves=BigInt(unit.value.split('.')[0]!)*2n+(unit.value.endsWith('.5')?1n:0n);
    const placement=model.PhysicalPlacement.create({item:equipment.value.target,enclosure:props.target,volume:{
      x:{startUm:rack.x.startUm+(lane.value==='right'?BigInt(rack.x.lengthUm)-BigInt(v.x.lengthUm):0n),lengthUm:v.x.lengthUm},
      y:{startUm:rack.y.startUm+(halves-2n)*22225n,lengthUm:v.y.lengthUm},
      z:{startUm:rack.z.startUm+(side.value==='rear'?BigInt(rack.z.lengthUm)-BigInt(v.z.lengthUm):0n),lengthUm:v.z.lengthUm},
    }});
    const placements=plan.value?.placements??[];
    if(placements.length>=64)throw Error('One plan supports at most 64 placements.');
    if(placements.some(p=>key(p.item)===key(placement.item)))throw Error('This equipment is already in the plan.');
    plan.value=model.PhysicalPlan.create({...plan.value,name:name.value,state:model.PhysicalPlanState.DRAFT,placements:[...placements,placement]});
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
    if(!plan.value||!selected.value.length)throw Error('Select the equipment actually installed.');
    const installed=plan.value.placements.filter(p=>selected.value.includes(key(p.item))).map(p=>p.item!);
    const reply=await invoke({oneofKind:'complete',complete:{plan:planKey(),installed,discrepancy:discrepancy.value||undefined}});
    plan.value=reply.plan;selected.value=[];notice.value='Reported installation recorded. Live health has not been asserted.';await refresh();await summaries();
  });
}
async function cancel() {
  await run(async()=>{const reply=await invoke({oneofKind:'cancel',cancel:planKey()});plan.value=reply.plan;notice.value='Draft cancelled.';await summaries();});
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
const local=computed(()=>plan.value?.placements.filter(p=>key(p.enclosure)===key(props.target))??[]);
</script>
<template>
<details class="physical-planner" @toggle="($event.target as HTMLDetailsElement).open&&open()">
<summary>Plan installation from enclosing scope</summary>
<p>Choose equipment from the enclosing selection. A draft changes no actual placement. Reservations and reported installation are separate actions.</p>
<p v-if="busy" role="status">Updating physical plan…</p><p v-if="error" role="alert">{{error}}</p><p v-if="notice" role="status">{{notice}}</p>
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
<PlacementSearch kind="equipment" :roots="working().roots" @select="choose"/>
<p v-if="equipment">Selected: {{itemName(equipment)}}. {{definition?.volume?'Dimensions available.':'Dimensions are unknown.'}}</p>
<div class="placement-controls"><label>Starting U<input v-model="unit" inputmode="decimal" maxlength="5"/></label>
<label>Side<select v-model="side"><option value="front">Front</option><option value="rear">Rear</option></select></label>
<label>Horizontal position<select v-model="lane"><option value="left">Left</option><option value="right">Right</option></select></label></div>
<button type="button" :disabled="!definition?.volume" @click="add">Add planned placement</button>
<button type="submit" :disabled="!plan?.placements.length">Save installation plan</button>
</fieldset></form>
<template v-if="plan"><h3>{{plan.name||name}} <small>{{state(plan.state)}}</small></h3>
<ul class="planned-items"><li v-for="(p,index) in plan.placements" :key="key(p.item)">
<label v-if="installable"><input v-model="selected" type="checkbox" :value="key(p.item)"/>Actually installed: <span v-if="label(p.item)">{{label(p.item)}}</span><span v-else>Equipment name unavailable</span></label>
<span v-else-if="label(p.item)">{{label(p.item)}}</span><span v-else>Equipment name unavailable</span>
<small v-if="p.volume?.y">Y {{millimetres(p.volume.y.startUm)}} mm; height {{millimetres(p.volume.y.lengthUm)}} mm</small>
<button v-if="editable" type="button" :disabled="busy" @click="plan.placements.splice(index,1)">Remove planned placement</button>
</li></ul>
<form v-if="reservable" @submit.prevent="reserve"><label>Reserve until<input v-model="deadline" type="datetime-local" required/></label><button :disabled="busy" type="submit">Check and reserve capacity</button></form>
<form v-if="installable" @submit.prevent="complete"><label>Installation discrepancies<textarea v-model="discrepancy" maxlength="2048"/></label><p>Select only equipment actually installed. Other reservations remain unresolved.</p><button :disabled="busy||!selected.length" type="submit">Record selected installation</button></form>
<button v-if="reservable" :disabled="busy" type="button" @click="cancel">Cancel draft</button>
</template>
</section></div>
</template>
</details>
</template>
<style scoped>
.physical-planner{margin-block:1rem;min-width:0}.physical-planner fieldset{border:0;padding:0;min-width:0}.physical-planner label{display:grid;gap:.35rem;margin-block:.75rem}.physical-planner button,.physical-planner input,.physical-planner select{min-height:44px;max-width:100%;box-sizing:border-box}.physical-planner textarea{width:100%;min-height:90px;box-sizing:border-box}.physical-planner ul{padding-left:1rem}.physical-planner small{display:block;overflow-wrap:anywhere}.plan-layout{display:grid;grid-template-columns:minmax(180px,1fr) minmax(260px,2fr);gap:1.5rem}.plan-layout>section{min-width:0}.plan-layout svg{display:block;width:100%;max-height:660px;background:#eef3f3}.rack{fill:#fff;stroke:#365c65;stroke-width:2;vector-effect:non-scaling-stroke}.installed{fill:#a6c6ce;stroke:#244c58;stroke-width:2;vector-effect:non-scaling-stroke}.proposed{fill:#d9e5be66;stroke:#586b28;stroke-width:2;stroke-dasharray:5 4;vector-effect:non-scaling-stroke}.placement-controls{display:grid;grid-template-columns:repeat(3,minmax(0,1fr));gap:.5rem}.planned-items li{padding-block:.5rem;overflow-wrap:anywhere}.planned-items label{display:flex;align-items:center}.physical-planner section>ul button{text-align:left}@media(max-width:700px){.plan-layout{display:flex;flex-direction:column-reverse}.plan-layout svg{max-height:420px}.placement-controls{grid-template-columns:1fr}.physical-planner button{width:100%;margin-block:.25rem}}
</style>
