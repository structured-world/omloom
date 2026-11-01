<script setup lang="ts">
import { ref } from 'vue';
import { call, model, requestMessage, AssetKind, type AssetRef } from './api';
import { millimetres, micrometres, nonnegativeMicrometres, signedMicrometres } from './physical-units';
import MatingEditor from './MatingEditor.vue';
import {railGrid} from './rack-grid';
const props = defineProps<{target: AssetRef; kind:number; working: model.WorkingContext}>();
const emit = defineEmits<{saved: [];equipment:[]}>();
const current = ref<model.PhysicalDefinition>();
const revision = ref<bigint>();
const busy = ref(false);
const error = ref('');
const opened = ref(false);
const basis = ref('');
const width = ref('');
const height = ref('');
const depth = ref('');
const known = ref(false);
const maxMass=ref(''),demand=ref(''),capacity=ref(''),cooling=ref(''),coolingCapacity=ref(''),mounting=ref('');
const componentKind=ref(model.PhysicalComponentKind.UNSPECIFIED);
const assemblyKind=ref(model.PhysicalAssemblyKind.UNSPECIFIED);
const channels=ref('');
const mating=ref<model.PhysicalMating>();
const clearanceKnown=ref(false);
const occupancyKnown=ref(false);
const powerSourcesComplete=ref(false);
const gridKnown=ref(false),gridTop=ref(''),gridCount=ref(''),gridFirst=ref('');
const gridDirection=ref(model.PhysicalRackDirection.UNSPECIFIED);
const axes=['x','y','z'] as const;
const survey=ref({x:{start:'',length:''},y:{start:'',length:''},z:{start:'',length:''}});
function surveyInputs(value:model.PhysicalVolume|undefined){for(const axis of axes){const extent=value?.[axis];survey.value[axis]={start:extent===undefined?'':millimetres(extent.startUm),length:extent===undefined?'':millimetres(extent.lengthUm)};}}
function enteredVolume(){
  if(!known.value)return undefined;
  const extent=(length:string,previous?:model.PhysicalExtent):model.PhysicalExtent=>({startUm:previous===undefined?0n:previous.startUm,lengthUm:micrometres(length)});
  return model.PhysicalVolume.create({x:extent(width.value,current.value?.volume?.x),y:extent(height.value,current.value?.volume?.y),z:extent(depth.value,current.value?.volume?.z)});
}
function copySurvey(){try{const value=enteredVolume();if(value===undefined)throw Error('Enter the known usable region first.');surveyInputs(value);}catch(e){error.value=requestMessage(e);}}
const margins=ref<Record<keyof model.PhysicalClearance,string>>({beforeXUm:'',afterXUm:'',beforeYUm:'',afterYUm:'',beforeZUm:'',afterZUm:''});
const faces:{key:keyof model.PhysicalClearance;label:string}[]=[
  {key:'beforeXUm',label:'Before X'},{key:'afterXUm',label:'After X'},
  {key:'beforeYUm',label:'Before Y'},{key:'afterYUm',label:'After Y'},
  {key:'beforeZUm',label:'Before Z'},{key:'afterZUm',label:'After Z'},
];
const componentKinds=Object.values(model.PhysicalComponentKind).filter((v):v is number=>typeof v==='number');
async function load() {
  if (opened.value || busy.value) return;
  busy.value = true; error.value = '';
  try {
    const response = await call({oneofKind:'physical',physical:model.Physical.create({action:{oneofKind:'read',read:{enclosure:props.target,occupants:false}}})},props.working);
    if (response.oneofKind !== 'physical') throw Error('Physical facts are unavailable.');
    revision.value = response.physical.revision;
    current.value = response.physical.definitions[0];
    assemblyKind.value=current.value?.assemblyKind??model.PhysicalAssemblyKind.UNSPECIFIED;
    channels.value=current.value?.channels.join(', ')??'';
    powerSourcesComplete.value=current.value?.powerSourcesComplete===true;
    const grid=current.value?.rackGrid;
    gridKnown.value=grid!==undefined;
    gridTop.value=grid===undefined?'':millimetres(grid.topUm);
    gridCount.value=grid===undefined?'':String(grid.unitCount);
    gridFirst.value=grid===undefined?'':String(grid.firstUnit);
    gridDirection.value=grid===undefined?model.PhysicalRackDirection.UNSPECIFIED:grid.direction;
    mating.value=current.value?.mating===undefined?undefined:model.PhysicalMating.clone(current.value.mating);
    clearanceKnown.value=current.value?.clearance!==undefined;
    occupancyKnown.value=current.value?.occupancyCoverage!==undefined;
    surveyInputs(current.value?.occupancyCoverage);
    for(const face of faces)margins.value[face.key]=current.value?.clearance===undefined?'':millimetres(current.value.clearance[face.key]);
    basis.value = current.value?.basis ?? '';
    const number=(value:bigint|undefined)=>value===undefined?'':value.toString();
    maxMass.value=number(current.value?.maxMassG);
    demand.value=number(current.value?.demandMw);capacity.value=number(current.value?.capacityMw);
    cooling.value=number(current.value?.coolingMw);coolingCapacity.value=number(current.value?.coolingCapacityMw);
    mounting.value=current.value?.mounting??'';
    componentKind.value=current.value===undefined?model.PhysicalComponentKind.UNSPECIFIED:current.value.componentKind;
    const v = current.value?.volume;
    known.value = v !== undefined;
    if (v?.x && v.y && v.z) {
      width.value = millimetres(v.x.lengthUm);
      height.value = millimetres(v.y.lengthUm);
      depth.value = millimetres(v.z.lengthUm);
    }
    opened.value = true;
  } catch (e) { error.value = requestMessage(e); }
  finally { busy.value = false; }
}
async function save() {
  if (busy.value || revision.value === undefined) return;
  busy.value = true; error.value = '';
  try {
    const volume = enteredVolume();
    const quantity=(value:string)=>{
      if(value==='')return undefined;
      if(!/^(0|[1-9][0-9]{0,18})$/.test(value))throw Error('Enter a nonnegative integer or leave the quantity unknown.');
      const parsed=BigInt(value);if(parsed>9223372036854775807n)throw Error('The physical quantity exceeds the supported range.');
      return parsed;
    };
    const definition = model.PhysicalDefinition.create({...current.value,target:props.target,basis:basis.value,volume,componentKind:componentKind.value,maxMassG:quantity(maxMass.value),demandMw:quantity(demand.value),capacityMw:quantity(capacity.value),coolingMw:quantity(cooling.value),coolingCapacityMw:quantity(coolingCapacity.value),mounting:mounting.value===''?undefined:mounting.value});
    definition.mating=mating.value;
    definition.assemblyKind=assemblyKind.value;
    definition.channels=channels.value.trim()===''?[]:channels.value.split(',').map(value=>{const text=value.trim();if(!/^[1-9][0-9]{0,3}$/.test(text)||Number(text)>4096)throw Error('Use distinct sourced channel numbers from 1 to 4096.');return Number(text);});
    if(definition.channels.length>64||new Set(definition.channels).size!==definition.channels.length)throw Error('Use at most 64 distinct sourced channels.');
    definition.powerSourcesComplete=powerSourcesComplete.value;
    definition.rackGrid=undefined;
    if(gridKnown.value){
      const positive=(value:string)=>{if(!/^[1-9][0-9]{0,9}$/.test(value)||Number(value)>0xffffffff)throw Error('Enter a positive rack number within uint32.');return Number(value);};
      const grid=model.PhysicalRackGrid.create({topUm:signedMicrometres(gridTop.value),unitCount:positive(gridCount.value),firstUnit:positive(gridFirst.value),direction:gridDirection.value});
      railGrid(grid);definition.rackGrid=grid;
    }
    definition.clearance=clearanceKnown.value?model.PhysicalClearance.create(Object.fromEntries(faces.map(face=>[face.key,nonnegativeMicrometres(margins.value[face.key])]))):undefined;
    definition.occupancyCoverage=occupancyKnown.value?model.PhysicalVolume.create(Object.fromEntries(axes.map(axis=>[axis,{startUm:signedMicrometres(survey.value[axis].start),lengthUm:micrometres(survey.value[axis].length)}]))):undefined;
    const response = await call({oneofKind:'physical',physical:model.Physical.create({action:{oneofKind:'define',define:{definition}}})},{...props.working,expected:revision.value});
    if (response.oneofKind !== 'physical') throw Error('Physical facts were not saved.');
    current.value = response.physical.definitions[0];
    revision.value = response.physical.revision;
    emit('saved');
  } catch (e) { error.value = requestMessage(e); }
  finally { busy.value = false; }
}
</script>
<template>
<details class="physical-facts" @toggle="($event.target as HTMLDetailsElement).open && load()">
<summary>{{kind===AssetKind.POWER_FEED?'Power source coverage':'Usable enclosure space and physical constraints'}}</summary>
<template v-if="[AssetKind.DEVICE,AssetKind.PHYSICAL_COMPONENT].includes(kind)"><p>Equipment dimensions and mass belong to its equipment facts, including their source and uncertainty. Usable interior space is a separate measurement.</p><button type="button" @click="emit('equipment')">Edit equipment facts</button></template>
<p v-if="busy" role="status">Loading physical facts…</p>
<p v-if="error" role="alert">{{error}}</p>
<form v-if="opened" @submit.prevent="save">
  <fieldset :disabled="busy">
    <label>Measurement or specification source<input v-model="basis" required maxlength="512" /></label>
    <details v-if="[AssetKind.DEVICE,AssetKind.PHYSICAL_COMPONENT,AssetKind.PORT,AssetKind.POWER_FEED].includes(kind)"><summary>Recorded power sources</summary>
      <p>Record upstream supplies with Powered by relationships. This statement covers the current source list, not live power, electrical safety or capacity after a failure.</p>
      <label class="check"><input v-model="powerSourcesComplete" type="checkbox"/>All upstream power dependencies of this resource are recorded</label>
    </details>
    <template v-if="kind!==AssetKind.POWER_FEED">
    <label v-if="[AssetKind.DEVICE,AssetKind.PHYSICAL_COMPONENT].includes(kind)">Known physical class<select v-model="componentKind"><option v-for="kind in componentKinds" :key="kind" :value="kind">{{model.PhysicalComponentKind[kind]?.toLowerCase().replaceAll('_',' ')}}</option></select></label>
    <label class="check"><input v-model="known" type="checkbox" />Usable enclosure dimensions are known</label>
    <template v-if="known">
      <label>Width (mm)<input v-model="width" inputmode="decimal" required maxlength="20" /></label>
      <label>Height (mm)<input v-model="height" inputmode="decimal" required maxlength="20" /></label>
      <label>Depth (mm)<input v-model="depth" inputmode="decimal" required maxlength="20" /></label>
    </template>
    <p v-else>Unknown dimensions cannot establish physical fit.</p>
    <details v-if="kind===AssetKind.RACK"><summary>Mounting rail grid</summary>
      <p>The rail grid is independent of cabinet height and occupancy. Record its source above. Unit numbers do not instantiate inventory slots.</p>
      <label class="check"><input v-model="gridKnown" type="checkbox"/>Rack rail grid and numbering are known</label>
      <template v-if="gridKnown"><label>Rail grid top Y (mm)<input v-model="gridTop" required inputmode="decimal" maxlength="21"/></label>
      <label>Rail unit count<input v-model="gridCount" required inputmode="numeric" maxlength="10"/></label>
      <label>Lowest printed U<input v-model="gridFirst" required inputmode="numeric" maxlength="10"/></label>
      <label>Rack numbering direction<select v-model="gridDirection"><option :value="model.PhysicalRackDirection.UNSPECIFIED">Choose the sourced direction</option><option :value="model.PhysicalRackDirection.TOP_DOWN">Top to bottom</option><option :value="model.PhysicalRackDirection.BOTTOM_UP">Bottom to top</option></select></label></template>
    </details>
    <details v-if="kind!==AssetKind.PORT"><summary>Known current occupancy region</summary>
      <p>Dimensions and model slots do not prove empty space. Explicitly record the region whose current occupants are all represented in inventory. Other regions remain unqualified; saving this statement moves no equipment and releases no reservation. Include the review source in the source field above.</p>
      <label class="check"><input v-model="occupancyKnown" type="checkbox"/>Current occupancy is completely recorded in this region</label>
      <template v-if="occupancyKnown"><button type="button" @click="copySurvey">Copy entered usable region</button>
      <template v-for="axis in axes" :key="axis"><label>{{axis.toUpperCase()}} occupancy start (mm)<input v-model="survey[axis].start" required inputmode="decimal" maxlength="21"/></label><label>{{axis.toUpperCase()}} occupancy length (mm)<input v-model="survey[axis].length" required inputmode="decimal" maxlength="20"/></label></template></template>
    </details>
    <details><summary>Capacity and mounting facts</summary>
      <p>Empty means unknown. Zero is an explicit sourced value. Dimensions alone do not establish power, cooling or mounting compatibility.</p>
      <label>Enclosure mass limit (g)<input v-model="maxMass" inputmode="numeric" maxlength="19"/></label>
      <label>Equipment power demand (mW)<input v-model="demand" inputmode="numeric" maxlength="19"/></label>
      <label>Enclosure power capacity (mW)<input v-model="capacity" inputmode="numeric" maxlength="19"/></label>
      <label>Equipment heat load (mW)<input v-model="cooling" inputmode="numeric" maxlength="19"/></label>
      <label>Enclosure cooling capacity (mW)<input v-model="coolingCapacity" inputmode="numeric" maxlength="19"/></label>
      <label>Mounting specification<input v-model="mounting" maxlength="128"/></label>
    </details>
    <details v-if="[AssetKind.DEVICE,AssetKind.PHYSICAL_COMPONENT].includes(kind)"><summary>Equipment service and connector clearance</summary>
      <p>Declare all six margins in the enclosing coordinate frame, including explicit zeros. These are selected equipment requirements, not proof of an outside aisle or cable route.</p>
      <label class="check"><input v-model="clearanceKnown" type="checkbox"/>Required clearance measurements are known</label>
      <template v-if="clearanceKnown"><label v-for="face in faces" :key="face.key">{{face.label}} clearance (mm)<input v-model="margins[face.key]" inputmode="decimal" required maxlength="20"/></label></template>
    </details>
    </template>
    <button type="submit">Save physical facts</button>
    <label v-if="kind===AssetKind.CABLE">Assembly construction<select v-model="assemblyKind"><option :value="model.PhysicalAssemblyKind.UNSPECIFIED">Unknown</option><option :value="model.PhysicalAssemblyKind.PASSIVE">Passive cable assembly</option><option :value="model.PhysicalAssemblyKind.DAC">DAC, inseparable end modules</option><option :value="model.PhysicalAssemblyKind.AOC">AOC, inseparable end modules</option></select></label>
    <MatingEditor v-if="kind===AssetKind.PORT" v-model="mating"/>
    <label v-if="kind===AssetKind.PORT">Known supplied port channels<input v-model="channels" maxlength="384" placeholder="For example: 1, 2, 3, 4"/></label>
  </fieldset>
</form>
</details>
</template>
<style scoped>
.physical-facts{margin-block:1rem;min-width:0}.physical-facts fieldset{display:grid;gap:.75rem;border:0;padding:.75rem 0}.physical-facts input:not([type=checkbox]){width:100%;box-sizing:border-box}.physical-facts button{min-height:44px}.check{display:flex;align-items:center;gap:.5rem}
</style>
