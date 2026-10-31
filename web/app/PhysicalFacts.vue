<script setup lang="ts">
import { ref } from 'vue';
import { call, model, AssetKind, type AssetRef } from './api';
import { millimetres, micrometres } from './physical-units';
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
const componentKinds=Object.values(model.PhysicalComponentKind).filter((v):v is number=>typeof v==='number');
async function load() {
  if (opened.value || busy.value) return;
  busy.value = true; error.value = '';
  try {
    const response = await call({oneofKind:'physical',physical:model.Physical.create({action:{oneofKind:'read',read:{enclosure:props.target,occupants:false}}})},props.working);
    if (response.oneofKind !== 'physical') throw Error('Physical facts are unavailable.');
    revision.value = response.physical.revision;
    current.value = response.physical.definitions[0];
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
  } catch (e) { error.value = String(e); }
  finally { busy.value = false; }
}
async function save() {
  if (busy.value || revision.value === undefined) return;
  busy.value = true; error.value = '';
  try {
    const extent = (length: string, previous?: model.PhysicalExtent): model.PhysicalExtent => ({startUm:previous === undefined ? 0n : previous.startUm,lengthUm:micrometres(length)});
    const volume = known.value ? model.PhysicalVolume.create({x:extent(width.value,current.value?.volume?.x),y:extent(height.value,current.value?.volume?.y),z:extent(depth.value,current.value?.volume?.z)}) : undefined;
    const quantity=(value:string)=>{
      if(value==='')return undefined;
      if(!/^(0|[1-9][0-9]{0,18})$/.test(value))throw Error('Enter a nonnegative integer or leave the quantity unknown.');
      const parsed=BigInt(value);if(parsed>9223372036854775807n)throw Error('The physical quantity exceeds the supported range.');
      return parsed;
    };
    const definition = model.PhysicalDefinition.create({...current.value,target:props.target,basis:basis.value,volume,componentKind:componentKind.value,maxMassG:quantity(maxMass.value),demandMw:quantity(demand.value),capacityMw:quantity(capacity.value),coolingMw:quantity(cooling.value),coolingCapacityMw:quantity(coolingCapacity.value),mounting:mounting.value===''?undefined:mounting.value});
    const response = await call({oneofKind:'physical',physical:model.Physical.create({action:{oneofKind:'define',define:{definition}}})},{...props.working,expected:revision.value});
    if (response.oneofKind !== 'physical') throw Error('Physical facts were not saved.');
    current.value = response.physical.definitions[0];
    revision.value = response.physical.revision;
    emit('saved');
  } catch (e) { error.value = String(e); }
  finally { busy.value = false; }
}
</script>
<template>
<details class="physical-facts" @toggle="($event.target as HTMLDetailsElement).open && load()">
<summary>Usable enclosure space and physical constraints</summary>
<template v-if="[AssetKind.DEVICE,AssetKind.PHYSICAL_COMPONENT].includes(kind)"><p>Equipment dimensions and mass belong to its equipment facts, including their source and uncertainty. Usable interior space is a separate measurement.</p><button type="button" @click="emit('equipment')">Edit equipment facts</button></template>
<p v-if="busy" role="status">Loading physical facts…</p>
<p v-if="error" role="alert">{{error}}</p>
<form v-if="opened" @submit.prevent="save">
  <fieldset :disabled="busy">
    <label>Measurement or specification source<input v-model="basis" required maxlength="512" /></label>
    <label v-if="[AssetKind.DEVICE,AssetKind.PHYSICAL_COMPONENT].includes(kind)">Known physical class<select v-model="componentKind"><option v-for="kind in componentKinds" :key="kind" :value="kind">{{model.PhysicalComponentKind[kind]?.toLowerCase().replaceAll('_',' ')}}</option></select></label>
    <label class="check"><input v-model="known" type="checkbox" />Usable enclosure dimensions are known</label>
    <template v-if="known">
      <label>Width (mm)<input v-model="width" inputmode="decimal" required maxlength="17" /></label>
      <label>Height (mm)<input v-model="height" inputmode="decimal" required maxlength="17" /></label>
      <label>Depth (mm)<input v-model="depth" inputmode="decimal" required maxlength="17" /></label>
    </template>
    <p v-else>Unknown dimensions cannot establish physical fit.</p>
    <details><summary>Capacity and mounting facts</summary>
      <p>Empty means unknown. Zero is an explicit sourced value. Dimensions alone do not establish power, cooling or mounting compatibility.</p>
      <label>Enclosure mass limit (g)<input v-model="maxMass" inputmode="numeric" maxlength="19"/></label>
      <label>Equipment power demand (mW)<input v-model="demand" inputmode="numeric" maxlength="19"/></label>
      <label>Enclosure power capacity (mW)<input v-model="capacity" inputmode="numeric" maxlength="19"/></label>
      <label>Equipment heat load (mW)<input v-model="cooling" inputmode="numeric" maxlength="19"/></label>
      <label>Enclosure cooling capacity (mW)<input v-model="coolingCapacity" inputmode="numeric" maxlength="19"/></label>
      <label>Mounting specification<input v-model="mounting" maxlength="128"/></label>
    </details>
    <button type="submit">Save physical facts</button>
  </fieldset>
</form>
</details>
</template>
<style scoped>
.physical-facts{margin-block:1rem;min-width:0}.physical-facts fieldset{display:grid;gap:.75rem;border:0;padding:.75rem 0}.physical-facts input:not([type=checkbox]){width:100%;box-sizing:border-box}.physical-facts button{min-height:44px}.check{display:flex;align-items:center;gap:.5rem}
</style>
