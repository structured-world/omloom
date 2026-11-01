<script setup lang="ts">
import { computed, ref, watch } from 'vue';
import { call, input, key, model, AssetKind as K, type AssetRef } from './api';
import PlacementSearch from './PlacementSearch.vue';
import PlanStepControls from './PlanStepControls.vue';
import PhysicalMaterials from './PhysicalMaterials.vue';
import EndpointMappings from './EndpointMappings.vue';
import ReplacementReview from './ReplacementReview.vue';
import { millimetres,micrometres,signedMicrometres,upperMicrometres } from './physical-units';
import {frameSize,projectElevation,depthOrder,type Face} from './physical-elevation';
import {rackAllocation,rackUnitAt,adjacentRackUnit} from './rack-grid';
const props = defineProps<{target:AssetRef;kind:number;working:model.WorkingContext;inactive?:boolean}>();
const emit=defineEmits<{changed:[]}>();
const opened=ref(false),busy=ref(false),error=ref(''),notice=ref('');
const view=ref(model.PhysicalView.create());
const revision=ref(props.working.expected);
const plans=ref<model.PhysicalPlanSummary[]>([]),next=ref<string>();
const plan=ref<model.PhysicalPlan>(),name=ref(''),deadline=ref(''),discrepancy=ref('');
const savedCurrent=ref(false);
function acceptPlan(value:model.PhysicalPlan|undefined){if(!value)throw Error('The saved plan was not returned. Reload before continuing.');plan.value=value;name.value=value.name;savedCurrent.value=true;}
const assessment=ref<{state:number;reason:number;revision:bigint}>();
const reviewHypotheses=ref(new Map<number,model.PhysicalReplacementHypothesis>());
const quality=ref<model.PhysicalReplacementQuality[]>(),qualityNext=ref<string>();
function clearQuality(){quality.value=undefined;qualityNext.value=undefined;}
const qualityScope=computed(()=>props.working.roots.map(key).sort().join('|'));
watch(qualityScope,clearQuality);
watch(()=>plan.value?.id,()=>{reviewHypotheses.value.clear();});
async function reviewReplacement(index:number,decision?:model.PhysicalReplacementDecision,reason=model.PhysicalReplacementReason.UNSPECIFIED,basis?:string){await run(async()=>{
  const reply=await invoke({oneofKind:'reviewReplacement',reviewReplacement:{plan:planKey('current'),step:index,decision,reason,basis}});
  acceptPlan(reply.plan);
  if(reply.replacementHypothesis)reviewHypotheses.value.set(index,reply.replacementHypothesis);
  if(decision!==undefined){notice.value='Review decision recorded. Installation and endpoint decisions are unchanged.';clearQuality();}
});}
async function loadQuality(after?:string){await run(async()=>{const scope=qualityScope.value;const reply=await invoke({oneofKind:'replacementQuality',replacementQuality:{after}});if(scope!==qualityScope.value)return;quality.value=reply.replacementQuality;qualityNext.value=reply.next;});}
watch([plan,name],()=>{assessment.value=undefined;savedCurrent.value=false;},{deep:true,flush:'sync'});
const assessmentText=computed(()=>{
  if(!assessment.value)return '';
  if(assessment.value.state===model.RequirementState.SATISFIED)return 'Declared physical constraints satisfied. Nothing was saved or reserved.';
  if(assessment.value.state===model.RequirementState.FAILED)return 'Declared physical constraints fail. Nothing was saved or reserved.';
  return `Physical result unknown: ${model.RequirementReason[assessment.value.reason]?.toLowerCase().replaceAll('_',' ')}. Nothing was saved or reserved.`;
});
const planItems=ref<model.Item[]>([]);
const emptyMaterials:model.PhysicalMaterial[]=[];
const future=ref<model.PhysicalMaterialUnit>();
const materialUses=computed(()=>{const units:model.PhysicalMaterialUnit[]=[];for(const step of plan.value?.steps??[])if(step.action.oneofKind==='placement'&&step.action.placement.material)units.push(step.action.placement.material);return units;});
watch(()=>plan.value?.id,()=>{future.value=undefined;selected.value=[];notPerformed.value=[];});
function updateMaterials(materials:model.PhysicalMaterial[]){future.value=undefined;plan.value=model.PhysicalPlan.create({...plan.value,name:name.value,state:model.PhysicalPlanState.DRAFT,materials});}
function removeMaterial(index:number){
  if(!plan.value)return;
  if(materialUses.value.some(u=>u.material===index)){error.value='Remove the material’s planned placements before removing its requirement.';return;}
  plan.value.materials=plan.value.materials.filter((_,i)=>i!==index);
  for(const unit of materialUses.value)if(unit.material>index)unit.material--;
  future.value=undefined;
}
function selectedMaterial(item:model.Item){const index=planItems.value.findIndex(old=>key(old.target)===key(item.target));if(index===-1)planItems.value.push(item);else planItems.value[index]=item;}
async function receiveMaterial(material:number,allocation:model.PhysicalMaterialAllocation){await run(async()=>{
  const reply=await invoke({oneofKind:'receive',receive:{plan:planKey('current'),material,allocation}});
  acceptPlan(reply.plan);selected.value=[];notPerformed.value=[];notice.value='Actual material allocation recorded. Nothing was installed.';
});}
const equipment=ref<model.Item>(),equipmentVolume=ref<model.PhysicalVolume>();
const manualRegion=ref(false),allocation=ref({x:{start:'',length:''},y:{start:'',length:''},z:{start:'',length:''}});
const axes=['x','y','z'] as const;
const mode=ref<'placement'|'replacement'|'wiring'>(props.kind===K.CABLE?'wiring':props.kind===K.RACK?'placement':'replacement');
const cableEnds=ref<model.PhysicalCableEnd[]>([]),endsNext=ref<string>(),endKey=ref('');
const wiringPort=ref<model.Item>(),wiringState=ref<'port'|'loose'|'unknown'>('port');
const wiringIntent=ref(model.PhysicalWiringIntent.PRESERVE);
const predecessor=ref(''),removePart=ref(false),reseat=ref(false),moving=ref(false),source=ref<AssetRef>();
const sourceView=ref<model.PhysicalView>();
const oldOccupancy=computed(()=>sourceView.value?.occupancy??view.value.occupancy);
const actualReport=ref(false),notPerformed=ref<number[]>([]);
const unit=ref('1'),side=ref<Face>('front'),lane=ref('left'),selected=ref<number[]>([]);
function resetReport(){actualReport.value=false;selected.value=[];notPerformed.value=[];discrepancy.value='';}
function newPlan(){plan.value=undefined;name.value='';resetReport();}
const elevation=ref<SVGSVGElement>();
const coordinatesOpen=ref(false);
const coordinateRows=computed(()=>view.value.occupancy);
function coordinate(value:model.PhysicalExtent|undefined){return value===undefined?'Unavailable':`${millimetres(value.startUm)} + ${millimetres(value.lengthUm)}`;}
const working=()=>model.WorkingContext.create({roots:props.working.roots,expected:revision.value});
const editable=computed(()=>plan.value===undefined||plan.value.state===model.PhysicalPlanState.DRAFT);
const geometryEditable=computed(()=>mode.value==='placement'||(mode.value==='replacement'&&props.kind===K.RACK&&!removePart.value));
const reservable=computed(()=>plan.value?.state===model.PhysicalPlanState.DRAFT);
const installable=computed(()=>plan.value!==undefined&&([model.PhysicalPlanState.RESERVED,model.PhysicalPlanState.PARTIAL,model.PhysicalPlanState.CANCELLED].includes(plan.value.state)||(plan.value.state===model.PhysicalPlanState.DRAFT&&actualReport.value)));
const envelope=computed(()=>view.value.definitions[0]?.volume);
const rackGrid=computed(()=>view.value.definitions[0]?.rackGrid);
const state=(value:number)=>model.PhysicalPlanState[value]?.toLowerCase();
const itemName=(item:model.Item)=>input(item.fields.find(f=>f.name==='name'));
const labels=computed(()=>{
  const result=new Map<string,string|undefined>();
  for(const items of [sourceView.value?.items,view.value.items,planItems.value]){
    if(items===undefined)continue;
    for(const item of items){if(!item.target)continue;const field=item.fields.find(f=>f.name==='name');result.set(key(item.target),field===undefined?undefined:input(field));}
  }
  return result;
});
const label=(target:AssetRef|undefined)=>target===undefined?undefined:labels.value.get(key(target));
async function invoke(action:model.Physical['action']) {
  const response=await call({oneofKind:'physical',physical:{action}},working());
  if(response.oneofKind!=='physical')throw Error('Physical planning is unavailable.');
  if((action.oneofKind==='save'||action.oneofKind==='reserve'||action.oneofKind==='complete'||action.oneofKind==='cancel'||action.oneofKind==='receive'||action.oneofKind==='reviewReplacement')&&response.physical.revision!==revision.value)changed=true;
  if(response.physical.revision!==revision.value){assessment.value=undefined;reviewHypotheses.value.clear();clearQuality();}
  revision.value=response.physical.revision;
  if(response.physical.plan)planItems.value=response.physical.items;
  return response.physical;
}
let changed=false;
async function run(action:()=>Promise<void>) {
  if(busy.value||props.inactive)return;
  changed=false;
  busy.value=true;error.value='';notice.value='';
  // Refresh before enabling actions so the parent refresh cannot disable the next click.
  try{await action();if(changed&&view.value.revision!==revision.value)await refresh(false);}catch(e){error.value=String(e);}finally{busy.value=false;if(changed)emit('changed');}
}
async function summaries(after?:string) {
  const reply=await invoke({oneofKind:'list',list:{after,limit:25}});
  plans.value=reply.plans;next.value=reply.next;
}
async function refresh(resetGrid=true,after?:string) {
  const reply=await invoke({oneofKind:'read',read:{enclosure:props.target,occupants:true,after}});
  view.value=reply;
  if(resetGrid&&rackGrid.value)unit.value=String(rackGrid.value.firstUnit);
  if(props.kind===K.CABLE)await loadEnds();
}
let parentBasis=props.working.expected;
watch([()=>props.working.expected,()=>props.inactive,busy],async()=>{
  if(props.inactive){parentBasis=undefined;return;}
  if(!opened.value||busy.value||parentBasis===props.working.expected)return;
  parentBasis=props.working.expected;
  // A current read can outrun its parent; it must not start a refresh loop.
  if(props.working.expected===view.value.revision)return;
  const acknowledged=notice.value;
  await run(async()=>{try{assessment.value=undefined;await refresh(false);await summaries();}finally{notice.value=acknowledged;}});
});
async function loadEnds(after?:string){const reply=await invoke({oneofKind:'cableEnds',cableEnds:{cable:props.target,after}});cableEnds.value=reply.cableEnds;endsNext.value=reply.next;}
function addWiring(){
  error.value='';
  const end=cableEnds.value.find(e=>e.key===endKey.value);
  if(!end){error.value='Choose a known end of this cable.';return;}
  if(wiringState.value==='port'&&!wiringPort.value?.target){error.value='Choose the actual physical connector.';return;}
  const steps=plan.value?.steps??[];
  if(steps.length>=64){error.value='One plan supports at most 64 physical steps.';return;}
  let prior:number|undefined;
  for(let i=0;i<steps.length;i++){const action=steps[i]!.action;if(action.oneofKind==='wiring'&&key(action.wiring.cable)===key(props.target)&&action.wiring.endKey===end.key)prior=i;}
  const wiring=model.PhysicalWiring.create({cable:props.target,endKey:end.key,intent:wiringIntent.value,
    basis:prior===undefined?{oneofKind:'expectedEndRevision',expectedEndRevision:end.revision}:{oneofKind:'afterStep',afterStep:prior},
    after:wiringState.value==='port'?{oneofKind:'port',port:wiringPort.value!.target!}:wiringState.value==='loose'?{oneofKind:'loose',loose:{}}:{oneofKind:'unknown',unknown:{}}});
  plan.value=model.PhysicalPlan.create({...plan.value,name:name.value,state:model.PhysicalPlanState.DRAFT,steps:[...steps,model.PhysicalStep.create({action:{oneofKind:'wiring',wiring}})]});
  if(wiringPort.value&&!planItems.value.some(i=>key(i.target)===key(wiringPort.value?.target)))planItems.value.push(wiringPort.value);
}
async function open() {
  if(opened.value)return;
  await run(async()=>{await refresh();await summaries();opened.value=true;});
}
async function load(saved:model.PhysicalPlanSummary) {
  await run(async()=>{
    const reply=await invoke({oneofKind:'load',load:{id:saved.id,revision:saved.revision}});
    acceptPlan(reply.plan);resetReport();
  });
}
let selection=0;
async function choose(item?:model.Item) {
  const generation=++selection;future.value=undefined;equipment.value=undefined;equipmentVolume.value=undefined;manualRegion.value=false;
  for(const axis of axes)allocation.value[axis]={start:'',length:''};
  if(!item?.target)return;
  await run(async()=>{
    const reply=await invoke({oneofKind:'read',read:{enclosure:item.target,occupants:false}});
    if(generation!==selection)return;
    equipment.value=item;
    const fields=reply.items.find(candidate=>key(candidate.target)===key(item.target))?.fields;
    if(fields)equipmentVolume.value=dimensions(fields);
  });
}
function dimensions(fields:model.Item['fields']):model.PhysicalVolume|undefined {
    const upper=(name:string)=>{
      const field=fields.find(field=>field.name===name);
      if(field?.value.oneofKind!=='equipment'||field.value.equipment.assertion.oneofKind!=='quantity')return undefined;
      const value=upperMicrometres(field.value.equipment.assertion.quantity);
      return value===undefined||value===0n?undefined:{startUm:0n,lengthUm:value};
    };
    const x=upper('width'),y=upper('height'),z=upper('depth');
    if(x&&y&&z)return {x,y,z};
}
function chooseFuture(unit:model.PhysicalMaterialUnit){
  const material=plan.value?.materials[unit.material];if(!material)return;
  ++selection;future.value=unit;equipment.value=undefined;equipmentVolume.value=dimensions(material.specifications);manualRegion.value=false;mode.value='placement';
  for(const axis of axes)allocation.value[axis]={start:'',length:''};
}
function placementTarget(p:model.PhysicalPlacement){if(p.material){const m=plan.value?.materials[p.material.material];return m?.allocations.find(a=>a.unit===p.material!.unit)?.item;}return p.item;}
function placementKey(p:model.PhysicalPlacement){return p.material?`material/${p.material.material}/${p.material.unit}`:key(p.item);}
function placementTitle(p:model.PhysicalPlacement){if(p.material){const m=plan.value?.materials[p.material.material];if(m)return `${m.name} · requested unit ${p.material.unit+1}`;return undefined;}return label(p.item);}
async function chooseSource(item?:model.Item) {
  source.value=undefined;sourceView.value=undefined;predecessor.value='';
  if(!item?.target)return;
  await run(async()=>{sourceView.value=await invoke({oneofKind:'read',read:{enclosure:item.target,occupants:true}});source.value=item.target;});
}
async function sourcePage(after?:string){if(!source.value)return;await run(async()=>{sourceView.value=await invoke({oneofKind:'read',read:{enclosure:source.value,occupants:true,after}});predecessor.value='';});}
watch([moving,reseat,predecessor],([move,same,selected],[wasMoving,wasSame])=>{
  if(props.kind!==K.RACK)return;
  if(move||same){const items=sourceView.value?.items??view.value.items;void choose(items.find(item=>key(item.target)===selected));}
  else if(wasMoving||wasSame)void choose();
});
function proposedVolume():model.PhysicalVolume {
  if(manualRegion.value)return model.PhysicalVolume.create(Object.fromEntries(axes.map(axis=>[axis,{startUm:signedMicrometres(allocation.value[axis].start),lengthUm:micrometres(allocation.value[axis].length)}])));
  const v=equipmentVolume.value,rack=envelope.value;
  if(!v?.x||!v.y||!v.z||!rack?.x||!rack.y||!rack.z)throw Error('Use an explicit draft allocation when equipment or enclosure dimensions are unknown.');
  if(!/^[1-9][0-9]{0,9}(\.5)?$/.test(unit.value)||(!rackGrid.value&&Number(unit.value)>=1000))throw Error('Enter a supported whole or half-unit position within the recorded range.');
  const halves=BigInt(unit.value.split('.')[0]!)*2n+(unit.value.endsWith('.5')?1n:0n);
  const y=rackGrid.value?rackAllocation(rackGrid.value,unit.value,v.y.lengthUm):{startUm:rack.y.startUm+(halves-2n)*22225n,lengthUm:v.y.lengthUm};
  return {x:{startUm:rack.x.startUm+(lane.value==='right'?BigInt(rack.x.lengthUm)-BigInt(v.x.lengthUm):0n),lengthUm:v.x.lengthUm},y,z:{startUm:rack.z.startUm+(side.value==='rear'?BigInt(rack.z.lengthUm)-BigInt(v.z.lengthUm):0n),lengthUm:v.z.lengthUm}};
}
function proposePreviousAllocation(){
  const volume=oldOccupancy.value.find(o=>key(o.placement?.item)===predecessor.value)?.placement?.volume;
  if(moving.value||!volume?.x||!volume.y||!volume.z){error.value='The previous allocation in this same enclosure is unavailable.';return;}
  for(const axis of axes)allocation.value[axis]={start:millimetres(volume[axis]!.startUm),length:millimetres(volume[axis]!.lengthUm)};
  manualRegion.value=true;
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
    if(props.kind===K.RACK&&replacement)r.volume=proposedVolume();
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
    if(!equipment.value?.target&&!future.value)throw Error('Choose existing equipment or a requested material unit first.');
    const proposed=proposedVolume();
    const placement=model.PhysicalPlacement.create({item:future.value?undefined:equipment.value?.target,material:future.value,enclosure:props.target,volume:proposed});
    const steps=plan.value?.steps??[];
    if(steps.length>=64)throw Error('One plan supports at most 64 physical steps.');
    if(steps.some(s=>s.action.oneofKind==='placement'&&placementKey(s.action.placement)===placementKey(placement)))throw Error('This equipment or requested unit is already in the plan.');
    plan.value=model.PhysicalPlan.create({...plan.value,name:name.value,state:model.PhysicalPlanState.DRAFT,steps:[...steps,model.PhysicalStep.create({state:model.PhysicalStepState.PLANNED,action:{oneofKind:'placement',placement}})]});
    if(equipment.value&&!planItems.value.some(i=>key(i.target)===key(equipment.value?.target)))planItems.value.push(equipment.value);
  }catch(e){error.value=String(e);}
}
function addAtCoordinates(){if(mode.value==='placement')add();else if(mode.value==='replacement')addReplacement();}
async function save() {
  await run(async()=>{
    if(!plan.value)throw Error('Add a physical step or material requirement.');
    const reply=await invoke({oneofKind:'save',save:{plan:{...plan.value,name:name.value}}});
    acceptPlan(reply.plan);notice.value='Plan saved. Nothing is reserved or installed.';await summaries();
  });
}
async function preview() {
  await run(async()=>{
    if(!plan.value)throw Error('Add at least one physical transition.');
    const reply=await invoke({oneofKind:'preview',preview:{plan:{...plan.value,name:name.value}}});
    assessment.value={state:reply.state,reason:reply.reason,revision:reply.revision};
  });
}
function planKey(basis:'current'|'stop'):model.PhysicalPlanKey {
  if(!plan.value?.id)throw Error('Save this plan before changing its lifecycle.');
  if(basis==='current'&&!savedCurrent.value)throw Error('Save these draft changes before reviewing, reserving or reporting work.');
  return {id:plan.value.id,revision:plan.value.revision};
}
async function reserve() {
  await run(async()=>{
    const value=new Date(deadline.value).getTime();
    if(!Number.isFinite(value))throw Error('Select an explicit reservation deadline.');
    const reply=await invoke({oneofKind:'reserve',reserve:{plan:planKey('current'),untilMs:BigInt(value)}});
    acceptPlan(reply.plan);notice.value='Capacity reserved. Installation is not confirmed.';await summaries();
  });
}
async function complete() {
  await run(async()=>{
    if(!plan.value||(!selected.value.length&&!notPerformed.value.length))throw Error('Select the physical outcomes actually confirmed.');
    const reply=await invoke({oneofKind:'complete',complete:model.PhysicalCompletion.create({plan:planKey('current'),completedSteps:selected.value,notPerformedSteps:notPerformed.value,actualState:actualReport.value,discrepancy:discrepancy.value===''?undefined:discrepancy.value})});
    acceptPlan(reply.plan);selected.value=[];notPerformed.value=[];notice.value='Reported installation recorded. Live health has not been asserted.';await refresh();await summaries();
  });
}
async function recheck(){await run(async()=>{await refresh();const reply=await invoke({oneofKind:'complete',complete:model.PhysicalCompletion.create({plan:planKey('current')})});acceptPlan(reply.plan);notice.value='Postconditions rechecked. No physical work was repeated.';});}
function subject(step:model.PhysicalStep){return step.action.oneofKind==='placement'?placementTarget(step.action.placement):step.action.oneofKind==='replacement'?step.action.replacement.replacement??step.action.replacement.previous:step.action.oneofKind==='wiring'?step.action.wiring.cable:undefined;}
function subjectKind(step:model.PhysicalStep){const target=subject(step);return planItems.value.find(i=>key(i.target)===key(target))?.kind;}
function predecessors(index:number){return plan.value?.steps.slice(0,index).map(step=>({title:label(subject(step))===undefined?'Equipment name unavailable':label(subject(step))!,hasCheck:step.control?.check!==undefined}))??[];}
function removeStep(index:number){
  if(!plan.value)return;
  if(plan.value.steps.some(s=>s.control?.after.some(d=>d.step===index)||(s.action.oneofKind==='wiring'&&s.action.wiring.basis.oneofKind==='afterStep'&&s.action.wiring.basis.afterStep===index))){error.value='Change dependent step links before removing this step.';return;}
  plan.value.steps.splice(index,1);
  reviewHypotheses.value.clear();selected.value=[];notPerformed.value=[];
  for(const step of plan.value.steps)for(const dependency of step.control?.after??[])if(dependency.step>index)dependency.step--;
  for(const step of plan.value.steps)if(step.action.oneofKind==='wiring'&&step.action.wiring.basis.oneofKind==='afterStep'&&step.action.wiring.basis.afterStep>index)step.action.wiring.basis.afterStep--;
}
async function cancel() {
  await run(async()=>{const reply=await invoke({oneofKind:'cancel',cancel:planKey('stop')});acceptPlan(reply.plan);notice.value='Remaining work stopped. Reported facts and unresolved reservations are preserved.';await summaries();});
}
const drawing=computed(()=>frameSize(envelope.value));
function projections(placements:Iterable<model.PhysicalPlacement>){
  const frame=envelope.value;
  const result:{p:model.PhysicalPlacement;rect:{x:number;y:number;width:number;height:number};depth:bigint;name:string|undefined}[]=[];
  if(!frame)return result;
  for(const p of placements){if(!p.volume)continue;const projection=projectElevation(frame,p.volume,side.value);if(projection)result.push({p,...projection,name:placementTitle(p)});}
  return result.sort(depthOrder);
}
function* reportedPlacements(){for(const entry of view.value.occupancy)if(entry.placement)yield entry.placement;}
const reportedProjection=computed(()=>projections(reportedPlacements()));
function dropEquipment(event:DragEvent){
  if(!editable.value||manualRegion.value||!equipmentVolume.value||!elevation.value||event.dataTransfer?.getData('application/x-omloom-equipment')!==selectedKey())return;
  const svg=elevation.value,transform=svg.getScreenCTM();
  if(!transform)return;
  const point=new DOMPoint(event.clientX,event.clientY).matrixTransform(transform.inverse());
  const drawingValue=drawing.value;if(!drawingValue)return;
  try{
    if(rackGrid.value&&envelope.value?.y&&equipmentVolume.value.y){
      const chosen=rackUnitAt(rackGrid.value,envelope.value.y.startUm+BigInt(Math.round(point.y*1000)),equipmentVolume.value.y.lengthUm);
      if(chosen===undefined)throw Error('Choose a position inside the declared rail grid.');
      unit.value=chosen;
    }else{
      const u=Math.floor((point.y-drawingValue.y)/44.45)+1;
      if(u<1||u>999)throw Error('Choose a position inside the known enclosure.');
      unit.value=String(u);
    }
  }catch(e){error.value=String(e);return;}
  const fromFront=side.value==='front'?point.x:drawingValue.width-point.x;
  lane.value=fromFront<drawingValue.width/2?'left':'right';
  addAtCoordinates();
}
function selectedKey(){return future.value?`material/${future.value.material}/${future.value.unit}`:key(equipment.value?.target);}
function dragEquipment(event:DragEvent){if(event.dataTransfer&&(equipment.value?.target||future.value)){event.dataTransfer.effectAllowed='copy';event.dataTransfer.setData('application/x-omloom-equipment',selectedKey());}}
function rackKeyboard(event:KeyboardEvent){
  if(!editable.value||manualRegion.value)return;
  if(event.key==='Enter'){event.preventDefault();addAtCoordinates();return;}
  if(event.key==='ArrowUp'||event.key==='ArrowDown'){
    event.preventDefault();
    if(rackGrid.value){const height=equipmentVolume.value?.y?.lengthUm;if(height===undefined){error.value='Select equipment with a known height before choosing a rail position.';return;}try{unit.value=adjacentRackUnit(rackGrid.value,unit.value,height,event.key==='ArrowUp');}catch(e){error.value=String(e);}return;}
    const current=Number(unit.value);if(!Number.isFinite(current))return;
    unit.value=String(Math.min(999,Math.max(1,current+(event.key==='ArrowUp'?-1:1))));
  }else if(event.key==='ArrowLeft'||event.key==='ArrowRight'){event.preventDefault();const visualLeft=event.key==='ArrowLeft';lane.value=(side.value==='front'?visualLeft:!visualLeft)?'left':'right';}
}
const placementRows=computed(()=>plan.value?.steps.flatMap((step,index)=>step.action.oneofKind==='placement'?[{p:step.action.placement,index,state:step.state}]:[])??[]);
const replacementRows=computed(()=>plan.value?.steps.flatMap((step,index)=>step.action.oneofKind==='replacement'?[{r:step.action.replacement,index,state:step.state}]:[])??[]);
const mappingCount=computed(()=>replacementRows.value.reduce((count,row)=>count+row.r.endpoints.length,0));
function outstanding(state:number){return state!==model.PhysicalStepState.RECORDED&&state!==model.PhysicalStepState.NOT_PERFORMED;}
const local=computed(()=>[
  ...placementRows.value.filter(row=>outstanding(row.state)&&key(row.p.enclosure)===key(props.target)).map(row=>row.p),
  ...replacementRows.value.filter(row=>outstanding(row.state)&&row.r.volume&&key(row.r.position)===key(props.target)).map(row=>model.PhysicalPlacement.create({item:row.r.replacement,enclosure:row.r.position,volume:row.r.volume})),
]);
const plannedProjection=computed(()=>projections(local.value));
const bill=computed(()=>{
  const items=new Map<string,{target:AssetRef;steps:model.PhysicalStep[]}>();
  for(const step of plan.value?.steps??[]){
    const targets:AssetRef[]=[];
    if(step.action.oneofKind==='placement'){const target=placementTarget(step.action.placement);if(target)targets.push(target);}
    if(step.action.oneofKind==='replacement'&&step.action.replacement.replacement)targets.push(step.action.replacement.replacement);
    if(step.action.oneofKind==='wiring'&&step.action.wiring.cable)targets.push(step.action.wiring.cable);
    for(const target of targets){let line=items.get(key(target));if(!line){line={target,steps:[]};items.set(key(target),line);}line.steps.push(step);}
  }
  for(const material of plan.value?.materials??[])for(const allocation of material.allocations){const target=allocation.item;if(target&&!items.has(key(target)))items.set(key(target),{target,steps:[]});}
  return [...items.values()].map(line=>({...line,reported:line.steps.filter(s=>s.state===model.PhysicalStepState.RECORDED).length,reserved:line.steps.some(s=>s.state===model.PhysicalStepState.RESERVED)}));
});
function serial(target:AssetRef){return planItems.value.find(i=>key(i.target)===key(target))?.fields.find(f=>f.name==='serial');}
async function exportPlan(){await run(async()=>{
  const response=await invoke({oneofKind:'load',load:planKey('current')});
  if(!response.plan)throw Error('The selected plan is unavailable.');
  const data={format:'omloom.physical-plan.v1',plan:model.PhysicalPlan.toJson(response.plan),items:response.items.map(item=>model.Item.toJson(item))};
  const url=URL.createObjectURL(new Blob([JSON.stringify(data,null,2)],{type:'application/json'}));
  try{const link=document.createElement('a');link.href=url;link.download='physical-plan.json';link.click();}finally{URL.revokeObjectURL(url);}
});}
</script>
<template>
<details class="physical-planner" :inert="inactive" @toggle="($event.target as HTMLDetailsElement).open&&open()">
<summary>Plan installation from enclosing scope</summary>
<p>Choose equipment from the enclosing selection. A draft changes no actual placement. Reservations and reported installation are separate actions.</p>
<p v-if="busy||notice||assessment" role="status">{{busy?'Updating physical plan…':notice?notice:assessmentText}}</p><p v-if="error" role="alert">{{error}}</p>
<p v-if="assessment">Checked at inventory revision {{assessment.revision.toString()}}. Reservation checks the current state again; this is not authority or proof of installation.</p>
<template v-if="opened">
<section aria-label="Saved physical plans"><h3>My plans</h3>
<button type="button" :disabled="busy" @click="newPlan">New installation plan</button>
<ul><li v-for="saved in plans" :key="saved.id"><button type="button" :disabled="busy" @click="load(saved)">{{saved.name}} · {{state(saved.state)}}</button></li></ul>
<button v-if="next" type="button" :disabled="busy" @click="run(()=>summaries(next))">Next saved plans</button>
</section>
<div class="plan-layout">
<section aria-label="Rack elevation"><template v-if="drawing">
<h3>Physical elevation</h3><p>Solid bodies are ordered by depth from the chosen face. Dashed proposals are overlays, not actual presence. Opposing sides can share U positions when depth permits.</p>
<div class="placement-controls"><button type="button" :aria-pressed="side==='front'" @click="side='front'">Front elevation</button><button type="button" :aria-pressed="side==='rear'" @click="side='rear'">Rear elevation</button></div>
<p>{{side==='front'?'Front':'Rear'}} placement context. Coordinates use the recorded enclosure origin. Arrow keys select U/side, Enter adds the selected equipment to the draft.</p>
<svg ref="elevation" role="group" tabindex="0" aria-label="Rack physical placement" :viewBox="`${drawing.x} ${drawing.y} ${drawing.width} ${drawing.height}`" @dragover.prevent @drop.prevent="dropEquipment" @keydown="rackKeyboard">
<rect :x="drawing.x" :y="drawing.y" :width="drawing.width" :height="drawing.height" class="rack"/>
<rect v-for="entry in reportedProjection" :key="key(entry.p.item)" v-bind="entry.rect" class="installed"><title>Reported installation: <template v-if="entry.name">{{entry.name}}</template><template v-else>Equipment name unavailable</template></title></rect>
<rect v-for="entry in plannedProjection" :key="placementKey(entry.p)" v-bind="entry.rect" class="proposed"><title>Planned placement: <template v-if="entry.name">{{entry.name}}</template><template v-else>Equipment name unavailable</template></title></rect>
</svg>
</template><p v-else>Known enclosure geometry is needed for an elevation. Available recorded coordinates and draft editing remain accessible.</p>
<p>Showing one page of permitted installed parts, not a declaration of complete occupancy or free capacity.</p>
<button type="button" :disabled="busy" @click="run(()=>refresh(false))">First installed-part page</button><button v-if="view.next" type="button" :disabled="busy" @click="run(()=>refresh(false,view.next))">Next installed-part page</button>
<p v-if="reportedProjection.length<view.occupancy.length">Some placements have no drawable coordinates in this view. The coordinate table preserves their available facts.</p>
<details v-if="view.occupancy.length" @toggle="coordinatesOpen=($event.target as HTMLDetailsElement).open"><summary>Reported placement coordinates</summary>
<template v-if="coordinatesOpen"><p>Start + length in millimetres in the recorded enclosure frame. Changing the viewing face changes no physical coordinates.</p>
<div class="coordinate-table" tabindex="0" role="region" aria-label="Reported coordinate table"><table><thead><tr><th>Equipment</th><th>X (mm)</th><th>Y (mm)</th><th>Z (mm)</th></tr></thead><tbody><tr v-for="(entry,index) in coordinateRows" :key="index"><th><template v-if="label(entry.placement?.item)">{{label(entry.placement?.item)}}</template><template v-else>Equipment name unavailable</template></th><td v-for="axis in axes" :key="axis">{{coordinate(entry.placement?.volume?.[axis])}}</td></tr></tbody></table></div>
</template>
</details>
</section>
<section aria-label="Installation plan editor">
<PhysicalMaterials :materials="plan?.materials??emptyMaterials" :uses="materialUses" :items="planItems" :working="working()" :editable="editable" :receivable="editable||plan?.state===model.PhysicalPlanState.RESERVED||plan?.state===model.PhysicalPlanState.PARTIAL" :can-place="kind===K.RACK" :busy="busy" @update="updateMaterials" @receive="receiveMaterial" @selected="selectedMaterial" @place="chooseFuture" @remove="removeMaterial"/>
<form v-if="editable" @submit.prevent="save"><fieldset :disabled="busy">
<label>Installation plan name<input v-model="name" required maxlength="160" /></label>
<div class="placement-controls"><button v-if="kind===K.RACK" type="button" :aria-pressed="mode==='placement'" @click="mode='placement'">Plan rack placement</button><button v-if="kind!==K.CABLE" type="button" :aria-pressed="mode==='replacement'" @click="mode='replacement'">Replace, remove or move a part</button></div>
<template v-if="mode==='placement'">
<PlacementSearch kind="equipment" :roots="working().roots" @select="choose"/>
</template>
<template v-else-if="mode==='replacement'">
<label class="outcome"><input v-model="moving" type="checkbox" @change="sourceView=undefined;source=undefined;predecessor=''"/>Move an established part from another enclosure</label>
<PlacementSearch v-if="moving" kind="enclosure" :roots="working().roots" @select="chooseSource"/>
<template v-if="moving&&source"><button type="button" :disabled="busy" @click="sourcePage()">First source-part page</button><button v-if="sourceView?.next" type="button" :disabled="busy" @click="sourcePage(sourceView.next)">Next source-part page</button></template>
<label>Known predecessor or moving part<select v-model="predecessor"><option value="">Predecessor unresolved, not a declaration of an empty position</option><option v-for="occupant in oldOccupancy" :key="key(occupant.placement?.item)" :value="key(occupant.placement?.item)"><template v-if="label(occupant.placement?.item)">{{label(occupant.placement?.item)}}</template><template v-else>Component name unavailable</template></option></select></label>
<template v-if="!moving">
<label class="outcome"><input v-model="removePart" type="checkbox" @change="reseat=false"/>Remove the known part without installing another</label>
<label class="outcome"><input v-model="reseat" type="checkbox" @change="removePart=false"/>Reinsert the same established part</label>
<PlacementSearch v-if="!removePart&&!reseat" :kind="kind===K.PHYSICAL_POSITION?'component':'equipment'" :roots="working().roots" @select="choose"/>
</template>
<p>The position and retained logical intent survive. Cable attachments, physical evidence, credentials and grants are not transferred to the incoming part.</p>
</template>
<template v-else>
<p>Cable work uses this enclosing selection and explicit endpoint choices. Unknown attachment retains possible occupancy; a confirmed loose end releases it. No service health is inferred.</p>
<label>Cable end<select v-model="endKey"><option value="">Choose a named end</option><option v-for="end in cableEnds" :key="end.key" :value="end.key">{{end.name}} · {{end.key}}</option></select></label>
<button v-if="endsNext" type="button" @click="run(()=>loadEnds(endsNext))">Next cable ends</button>
<label>Reported or planned attachment<select v-model="wiringState"><option value="port">Physical port</option><option value="loose">Confirmed loose</option><option value="unknown">Unknown attachment</option></select></label>
<PlacementSearch v-if="wiringState==='port'" kind="port" :roots="working().roots" @select="wiringPort=$event"/>
<label>Intended destination<select v-model="wiringIntent"><option :value="model.PhysicalWiringIntent.PRESERVE">Preserve existing intent</option><option v-if="wiringState==='port'" :value="model.PhysicalWiringIntent.ADOPT_DESTINATION">Adopt selected port as intent</option><option :value="model.PhysicalWiringIntent.CLEAR_DESTINATION">Explicitly clear intent</option></select></label>
<button type="button" @click="addWiring">Add cable-end transition</button>
</template>
<template v-if="geometryEditable">
<p v-if="equipment">Selected: {{itemName(equipment)}}. {{equipmentVolume?'Accepted upper-bound dimensions available.':'Accepted dimension bounds are unknown or unavailable.'}}</p>
<p v-if="future&&plan?.materials[future.material]">Selected requested material: {{plan.materials[future.material]!.name}}, unit {{future.unit+1}}. {{equipmentVolume?'Declared upper-bound dimensions available.':'Declare a draft allocation; material dimensions remain unknown.'}} This is not an adopted asset.</p>
<button v-if="mode==='replacement'&&!moving&&predecessor" type="button" @click="proposePreviousAllocation">Use previous allocation as a proposal</button>
<label v-if="equipment||future"><input v-model="manualRegion" type="checkbox"/>Use an explicit plan-only allocation</label>
<template v-if="manualRegion"><p>This is proposed space, not new equipment facts. Unknown source dimensions or occupancy keep fit unqualified. Confirm the recorded position and envelope when reporting actual installation.</p>
<template v-for="axis in axes" :key="axis"><label>{{axis.toUpperCase()}} allocation start (mm)<input v-model="allocation[axis].start" inputmode="decimal" maxlength="21"/></label><label>{{axis.toUpperCase()}} allocation length (mm)<input v-model="allocation[axis].length" inputmode="decimal" maxlength="20"/></label></template></template>
<template v-else><button v-if="equipmentVolume" type="button" draggable="true" @dragstart="dragEquipment" @click="addAtCoordinates">Drag selected equipment onto elevation, or add at chosen coordinates</button>
<p v-if="rackGrid">Uses the sourced rail grid. Allocation height rounds outward to half-U increments; equipment dimensions remain unchanged.</p><p v-else>No sourced rail grid. Positions below are geometric offsets from the usable top, not printed rack unit numbers.</p>
<div class="placement-controls"><label>{{rackGrid?'Starting U':'Position from usable top (U)'}}<input v-model="unit" inputmode="decimal" maxlength="12"/></label>
<label>Side<select v-model="side"><option value="front">Front</option><option value="rear">Rear</option></select></label>
<label>Horizontal position<select v-model="lane"><option value="left">Left from front</option><option value="right">Right from front</option></select></label></div></template>
</template>
<button v-if="mode==='placement'" type="button" :disabled="(!equipment?.target&&!future)||(!manualRegion&&!equipmentVolume)" @click="add">Add planned placement</button>
<button v-else-if="mode==='replacement'" type="button" @click="addReplacement">Add reviewed physical transition</button>
<button type="button" :disabled="!plan?.steps.length" @click="preview">Check physical requirements</button>
<button type="submit" :disabled="!plan||(!plan.steps.length&&!plan.materials.length)">Save installation plan</button>
</fieldset></form>
<template v-if="plan"><h3>{{plan.name||name}} <small>{{state(plan.state)}}</small></h3>
<section aria-label="Bill of materials"><h4>Selected serialized materials</h4><p>Each retained physical asset is counted once, including a component both moved and placed. Draft selection does not change stock or order equipment.</p>
<ul><li v-for="line in bill" :key="key(line.target)"><strong>{{label(line.target)??'Equipment name unavailable'}}</strong> · quantity 1<span v-if="serial(line.target)"> · serial {{input(serial(line.target))}}</span><small v-if="line.steps.length">{{line.reported}} reported of {{line.steps.length}} related steps. {{line.reserved?'Reservation held.':''}}</small></li></ul>
<button v-if="plan.id" type="button" :disabled="busy" @click="exportPlan">Export authorized plan and materials</button></section>
<ol class="planned-items"><li v-for="(step,index) in plan.steps" :key="index">
<template v-if="step.action.oneofKind==='placement'">
<label v-if="installable&&![model.PhysicalStepState.RECORDED,model.PhysicalStepState.NOT_PERFORMED].includes(step.state)"><input v-model="selected" type="checkbox" :value="index" :disabled="!placementTarget(step.action.placement)||notPerformed.includes(index)||(!actualReport&&step.readiness!==model.PlanReadiness.READY)"/>Actually installed: <span v-if="placementTitle(step.action.placement)">{{placementTitle(step.action.placement)}}</span><span v-else>Equipment name unavailable</span></label>
<span v-else-if="placementTitle(step.action.placement)">{{placementTitle(step.action.placement)}}</span><span v-else>Equipment name unavailable</span>
<p v-if="step.action.placement.material&&!placementTarget(step.action.placement)">Receive or select the actual requested unit before reporting installation.</p>
<template v-for="axis in axes" :key="axis"><small v-if="step.action.placement.volume?.[axis]">{{axis.toUpperCase()}} allocation: {{millimetres(step.action.placement.volume[axis]!.startUm)}} mm + {{millimetres(step.action.placement.volume[axis]!.lengthUm)}} mm</small></template>
</template>
<template v-if="step.action.oneofKind==='replacement'">
<EndpointMappings :replacement="step.action.replacement" :editable="editable&&!busy" :remaining="64-mappingCount" :items="planItems" @change="step.action.replacement.endpoints=$event" @selected="selectedMaterial"/>
<ReplacementReview :hypothesis="reviewHypotheses.get(index)" :feedback="step.action.replacement.feedback" :disabled="busy" :saved="!!plan.id&&savedCurrent" @inspect="reviewReplacement(index)" @judge="(decision,reason,basis)=>reviewReplacement(index,decision,reason,basis)"/>
<p><template v-if="step.action.replacement.previous">Previous: <span v-if="label(step.action.replacement.previous)">{{label(step.action.replacement.previous)}}</span><span v-else>Component name unavailable</span>.</template><template v-else>Predecessor unresolved.</template> <template v-if="step.action.replacement.replacement">Incoming: <span v-if="label(step.action.replacement.replacement)">{{label(step.action.replacement.replacement)}}</span><span v-else>Component name unavailable</span>.</template><template v-else>Removal without an incoming part.</template></p>
<label v-if="installable&&![model.PhysicalStepState.RECORDED,model.PhysicalStepState.NOT_PERFORMED].includes(step.state)"><input v-model="selected" type="checkbox" :value="index" :disabled="notPerformed.includes(index)||(!actualReport&&step.readiness!==model.PlanReadiness.READY)"/>Physical transition performed</label>
</template>
<small>{{model.PhysicalStepState[step.state]?.toLowerCase().replaceAll('_',' ')}}</small>
<template v-if="step.action.oneofKind==='wiring'"><p>End {{step.action.wiring.endKey}}: {{step.action.wiring.after.oneofKind==='port'?'connect to '+(label(step.action.wiring.after.port)??'selected physical port'):step.action.wiring.after.oneofKind==='loose'?'confirmed loose':'attachment unknown'}}. Intent: {{model.PhysicalWiringIntent[step.action.wiring.intent]?.toLowerCase().replaceAll('_',' ')}}.</p><label v-if="installable&&![model.PhysicalStepState.RECORDED,model.PhysicalStepState.NOT_PERFORMED].includes(step.state)"><input v-model="selected" type="checkbox" :value="index" :disabled="notPerformed.includes(index)||(!actualReport&&step.readiness!==model.PlanReadiness.READY)"/>Cable-end outcome reported</label></template>
<PlanStepControls :control="step.control" :index="index" :predecessors="predecessors(index)" :subject="subject(step)" :kind="subjectKind(step)" :working="working()" :editable="editable&&!busy" :readiness="step.readiness" :checked-state="step.checkedState" :checked-reason="step.checkedReason" @change="step.control=$event;step.readiness=model.PlanReadiness.UNSPECIFIED;step.checkedState=undefined;step.checkedReason=undefined"/>
<label v-if="installable&&![model.PhysicalStepState.RECORDED,model.PhysicalStepState.NOT_PERFORMED].includes(step.state)"><input v-model="notPerformed" type="checkbox" :value="index" :disabled="selected.includes(index)"/>Confirmed not performed</label>
<button v-if="editable" type="button" :disabled="busy" @click="removeStep(index)">Remove proposed step</button>
</li></ol>
<button v-if="plan.state===model.PhysicalPlanState.PARTIAL" type="button" :disabled="busy" @click="recheck">Recheck postconditions</button>
<label v-if="plan.state!==model.PhysicalPlanState.COMPLETED" class="outcome"><input v-model="actualReport" type="checkbox"/>Record physical work already performed outside Omloom</label>
<p v-if="plan.id&&!savedCurrent">Save these draft changes before reviewing, reserving or reporting work.</p>
<form v-if="reservable" @submit.prevent="reserve"><label>Reserve until<input v-model="deadline" type="datetime-local" required/></label><button :disabled="busy||!savedCurrent" type="submit">Check and reserve capacity</button></form>
<form v-if="installable" @submit.prevent="complete"><label>Installation discrepancies<textarea v-model="discrepancy" maxlength="2048"/></label><p>Record only physical outcomes actually known. Incompatibility may remain a discrepancy; this report does not verify wiring or service. Other reservations remain unresolved.</p><button :disabled="busy||!savedCurrent||(!selected.length&&!notPerformed.length)" type="submit">Record selected installation</button></form>
<button v-if="plan.state!==model.PhysicalPlanState.COMPLETED&&plan.state!==model.PhysicalPlanState.CANCELLED" :disabled="busy" type="button" @click="cancel">Stop remaining work</button>
</template>
<details><summary>Local replacement review quality</summary><p>Counts describe reviewed judgments in the current finite window, not predictor accuracy or recall. Deferred judgments are unlabelled. Unreviewed events and selection bias are not measured. The complete contributing read authorization and selected scope are required, including for an independent auditor.</p><button type="button" :disabled="busy" @click="loadQuality()">Load current review counts</button>
<p v-if="quality?.length===0">No retained judgments in this contributing scope.</p>
<ul v-if="quality"><li v-for="(bucket,index) in quality" :key="index">{{model.PhysicalComponentKind[bucket.componentKind]?.toLowerCase().replaceAll('_',' ')}} · rule {{bucket.rule}} · evidence class {{bucket.strength}} · {{model.PhysicalReplacementDecision[bucket.decision]?.toLowerCase()}} · {{model.PhysicalReplacementReason[bucket.reason]?.toLowerCase().replaceAll('_',' ')}}: {{bucket.count.toString()}} reviewed. Window ends {{new Date(Number(bucket.untilMs)).toLocaleString()}}.</li></ul><button v-if="qualityNext" :disabled="busy" type="button" @click="loadQuality(qualityNext)">Next review count buckets</button></details>
</section></div>
</template>
</details>
</template>
<style scoped>
.coordinate-table{max-width:100%;overflow:auto}.coordinate-table table{border-collapse:collapse;min-width:32rem}.coordinate-table th,.coordinate-table td{padding:.5rem;text-align:left;border-bottom:1px solid #cbd8da;overflow-wrap:anywhere}
.physical-planner{margin-block:1rem;min-width:0}.physical-planner fieldset{border:0;padding:0;min-width:0}.physical-planner label{display:grid;gap:.35rem;margin-block:.75rem}.physical-planner button,.physical-planner input,.physical-planner select{min-height:44px;max-width:100%;box-sizing:border-box}.physical-planner textarea{width:100%;min-height:90px;box-sizing:border-box}.physical-planner ul{padding-left:1rem}.physical-planner small{display:block;overflow-wrap:anywhere}.plan-layout{display:grid;grid-template-columns:minmax(180px,1fr) minmax(260px,2fr);gap:1.5rem}.plan-layout>section{min-width:0}.plan-layout svg{display:block;width:100%;max-height:660px;background:#eef3f3}.rack{fill:#fff;stroke:#365c65;stroke-width:2;vector-effect:non-scaling-stroke}.installed{fill:#a6c6ce;stroke:#244c58;stroke-width:2;vector-effect:non-scaling-stroke}.proposed{fill:#d9e5be66;stroke:#586b28;stroke-width:2;stroke-dasharray:5 4;vector-effect:non-scaling-stroke}.placement-controls{display:grid;grid-template-columns:repeat(3,minmax(0,1fr));gap:.5rem}.planned-items li{padding-block:.5rem;overflow-wrap:anywhere}.planned-items label{display:flex;align-items:center}.physical-planner section>ul button{text-align:left}@media(max-width:700px){.plan-layout{display:flex;flex-direction:column-reverse}.plan-layout svg{max-height:420px}.placement-controls{grid-template-columns:1fr}.physical-planner button{width:100%;margin-block:.25rem}}
.outcome{display:flex!important;align-items:center;gap:.5rem}
</style>
