<script setup lang="ts">
import { ref } from 'vue';
import { call,input,key,model,type AssetRef } from './api';
const props=defineProps<{target:AssetRef;working:model.WorkingContext}>();
const emit=defineEmits<{open:[target:AssetRef];changed:[]}>();
const view=ref(model.PhysicalView.create()),opened=ref(false),busy=ref(false),error=ref('');
const pages=ref<(string|undefined)[]>([undefined]);
const structural=ref(''),name=ref(''),kind=ref(model.PhysicalComponentKind.UNSPECIFIED),editing=ref<model.PhysicalPosition>();
const roles=Object.values(model.PhysicalComponentKind).filter((v):v is number=>typeof v==='number');
const role=(value:number)=>model.PhysicalComponentKind[value]?.toLowerCase().replaceAll('_',' ');
function title(target:AssetRef|undefined){const item=view.value.items.find(i=>key(i.target)===key(target));const field=item?.fields.find(f=>f.name==='name');return field===undefined?undefined:input(field);}
async function request(action:model.Physical['action']) {
  const response=await call({oneofKind:'physical',physical:{action}},{...props.working,expected:view.value.revision});
  if(response.oneofKind!=='physical')throw Error('Physical positions are unavailable.');
  return response.physical;
}
async function load(after?:string,backward=false) {
  if(busy.value)return;busy.value=true;error.value='';
  try{
    view.value=await request({oneofKind:'positions',positions:{owner:props.target,after,limit:25}});opened.value=true;
    if(backward)pages.value.pop();else if(after===undefined)pages.value=[undefined];else pages.value.push(after);
  }catch(e){error.value=String(e);}finally{busy.value=false;}
}
function edit(position?:model.PhysicalPosition){editing.value=position;structural.value=position?.structuralKey??'';name.value=title(position?.target)??'';kind.value=position?.componentKind??model.PhysicalComponentKind.UNSPECIFIED;}
async function save() {
  if(busy.value)return;busy.value=true;error.value='';
  try{
    await request({oneofKind:'position',position:{owner:props.target,structuralKey:structural.value,name:name.value,componentKind:kind.value,expected:editing.value?.revision}});
    emit('changed');
  }catch(e){error.value=String(e);}finally{busy.value=false;}
}
</script>
<template><details class="physical-positions" @toggle="($event.target as HTMLDetailsElement).open&&!opened&&load()">
<summary>Component positions and bays</summary>
<p>A structural bay belongs to this exact owner. Creating it does not create an installed disk, optic, power supply or other part.</p>
<p v-if="busy" role="status">Updating component positions…</p><p v-if="error" role="alert">{{error}}</p>
<template v-if="opened"><ul><li v-for="position in view.positions" :key="key(position.target)">
<button v-if="position.target" type="button" :disabled="busy" @click="emit('open',position.target)"><span v-if="title(position.target)">{{title(position.target)}}</span><span v-else>Position name unavailable</span></button>
<small>{{position.structuralKey}} · {{role(position.componentKind)}}</small><button type="button" :disabled="busy" @click="edit(position)">Edit position</button>
</li></ul>
<button v-if="pages.length>1" type="button" :disabled="busy" @click="load(pages[pages.length-2],true)">Previous component positions</button>
<button v-if="view.next" type="button" :disabled="busy" @click="load(view.next)">Next component positions</button>
<button type="button" :disabled="busy" @click="edit()">New structural position</button>
<form @submit.prevent="save"><fieldset :disabled="busy">
<label>Structural position key<input v-model="structural" :readonly="editing!==undefined" required maxlength="128" placeholder="For example: psu/1"/></label>
<label>Position name<input v-model="name" required maxlength="160"/></label>
<label>Declared component class<select v-model="kind"><option v-for="value in roles" :key="value" :value="value">{{role(value)}}</option></select></label>
<p>A declared class is not proof of form factor, media, electrical compatibility or present occupancy.</p>
<button type="submit">{{editing?'Save structural position':'Create structural position'}}</button>
</fieldset></form></template>
</details></template>
<style scoped>
.physical-positions{margin-block:1rem;min-width:0}.physical-positions fieldset{border:0;padding:0;display:grid;gap:.75rem}.physical-positions input,.physical-positions select{width:100%;box-sizing:border-box;min-height:44px}.physical-positions button{min-height:44px}.physical-positions li{margin-block:.75rem;overflow-wrap:anywhere}.physical-positions small{display:block}@media(max-width:700px){.physical-positions button{width:100%;margin-block:.25rem}}
</style>
