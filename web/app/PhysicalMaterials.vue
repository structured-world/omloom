<script setup lang="ts">
import {computed,ref,watch} from 'vue';
import {call,input,key,model,AssetKind as K} from './api';
import {equipmentFields} from './equipment-fields';
import type {Field} from '../src/generated/omloom/v1/contracts';
import PlacementSearch from './PlacementSearch.vue';
import EquipmentField from './EquipmentField.vue';
import {firstFreeUnit} from './material-unit';
const props=defineProps<{materials:model.PhysicalMaterial[];uses:model.PhysicalMaterialUnit[];items:model.Item[];working:model.WorkingContext;editable:boolean;receivable:boolean;canPlace:boolean;busy:boolean}>();
const emit=defineEmits<{update:[materials:model.PhysicalMaterial[]];remove:[material:number];place:[unit:model.PhysicalMaterialUnit];receive:[material:number,allocation:model.PhysicalMaterialAllocation];selected:[item:model.Item];create:[material:number,unit:number]}>();
const opened=ref(false),editing=ref(false),index=ref<number>(),error=ref(''),loading=ref(false);
const draft=ref(model.PhysicalMaterial.create()),quantity=ref(''),field=ref(equipmentFields[0]!),valid=ref(true);
const source=ref<model.Item>(),receiving=ref<number>(),unit=ref(''),basis=ref('');
const wider=ref(false);
const placing=ref<number>(),placementUnit=ref('');
const page=ref(0);
watch(()=>props.materials,(current,previous)=>{editing.value=false;if(receiving.value!==undefined){const line=receiving.value;const a=previous[line],b=current[line];if(!a||!b||!model.PhysicalMaterial.equals(a,b))receiving.value=undefined;}placing.value=undefined;page.value=0;});
const rows=computed(()=>props.materials.slice(page.value*25,(page.value+1)*25));
const names=computed(()=>{
  const result=new Map<string,string>();
  for(const item of props.items){const name=item.fields.find(f=>f.name==='name');if(name!==undefined)result.set(key(item.target),input(name));}
  return result;
});
const kinds=[K.DEVICE,K.PHYSICAL_COMPONENT,K.RACK,K.CABLE];
function start(line?:number){placing.value=undefined;receiving.value=undefined;index.value=line;draft.value=line===undefined?model.PhysicalMaterial.create({kind:K.DEVICE}):model.PhysicalMaterial.clone(props.materials[line]!);quantity.value=line===undefined?'':String(draft.value.quantity);field.value=equipmentFields[0]!;source.value=undefined;valid.value=true;editing.value=true;error.value='';}
function startReceiving(line:number){placing.value=undefined;editing.value=false;receiving.value=line;source.value=undefined;wider.value=false;const free=firstFreeUnit(props.materials[line]!);unit.value=free===undefined?'':String(free+1);basis.value='';}
function createReceived(){error.value='';try{const line=receiving.value;if(line===undefined)return;emit('create',line,integer(unit.value,props.materials[line]!.quantity)-1);}catch(e){error.value=String(e);}}
function integer(value:string,max:number){if(!/^\d+$/.test(value))throw Error('Enter a whole positive count.');const n=Number(value);if(!Number.isSafeInteger(n)||n<1||n>max)throw Error(`Enter a count between 1 and ${max}.`);return n;}
function claim(value:Field|undefined){draft.value.specifications=draft.value.specifications.filter(f=>f.name!==field.value);if(value)draft.value.specifications.push(value);}
async function copySpecification(){
  if(!source.value?.target)return;
  loading.value=true;error.value='';
  const owner=draft.value;const target=source.value.target;
  try{
    if(source.value.kind!==draft.value.kind)throw Error('The source resource must have the requested material kind.');
    const reply=await call({oneofKind:'physical',physical:{action:{oneofKind:'read',read:{enclosure:target,occupants:false}}}},props.working);
    if(!editing.value||owner!==draft.value)return;
    if(reply.oneofKind!=='physical')throw Error('Physical specification unavailable.');
    const item=reply.physical.items.find(i=>key(i.target)===key(target));
    draft.value.specifications=item?item.fields.filter(f=>equipmentFields.includes(f.name)&&f.value.oneofKind==='equipment'):[];
    const profile=reply.physical.definitions.find(d=>key(d.target)===key(target));
    draft.value.physical=profile?model.PhysicalDefinition.create({...profile,target:undefined,occupancyCoverage:undefined,powerSourcesComplete:false}):undefined;
    valid.value=true;
  }catch(e){error.value=String(e);}finally{loading.value=false;}
}
function commit(){error.value='';try{
  if(!draft.value.name.trim())throw Error('Name the material requirement.');
  if(!valid.value)throw Error('Complete or remove the selected specification.');
  const count=integer(quantity.value,4294967295);
  if(draft.value.allocations.some(a=>a.unit>=count))throw Error('The quantity cannot exclude an allocated unit.');
  if(index.value!==undefined&&props.uses.some(u=>u.material===index.value&&u.unit>=count))throw Error('Remove the affected planned placement before reducing its requested quantity.');
  const value=model.PhysicalMaterial.create({...draft.value,quantity:count});
  const next=props.materials.slice();
  if(index.value===undefined){if(next.length>=64)throw Error('One plan supports at most 64 material lines.');next.push(value);}else next[index.value]=value;
  emit('update',next);editing.value=false;
}catch(e){error.value=String(e);}}
function receive(line:number){error.value='';try{
  const material=props.materials[line];if(!material||!source.value?.target)throw Error('Choose the actual adopted resource.');
  if(source.value.kind!==material.kind)throw Error('The actual resource must have the requested material kind.');
  if(!basis.value.trim())throw Error('State the receipt or reuse basis.');
  const allocation=model.PhysicalMaterialAllocation.create({unit:integer(unit.value,material.quantity)-1,item:source.value.target,basis:basis.value});
  if(props.editable){
    const next=props.materials.map(m=>model.PhysicalMaterial.clone(m));
    const allocations=next[line]!.allocations;const old=allocations.findIndex(a=>a.unit===allocation.unit);
    if(old===-1){if(next.reduce((n,m)=>n+m.allocations.length,0)>=64)throw Error('One plan supports at most 64 real allocations.');allocations.push(allocation);}else allocations[old]=allocation;
    emit('update',next);
    emit('selected',source.value);
  }else emit('receive',line,allocation);
  receiving.value=undefined;
}catch(e){error.value=String(e);}}
function resource(target:model.PhysicalMaterialAllocation['item']){return names.value.get(key(target));}
function place(){error.value='';try{
  const index=placing.value;const material=index===undefined?undefined:props.materials[index];
  if(index===undefined||!material)throw Error('Choose a current material requirement.');
  const unit=integer(placementUnit.value,material.quantity)-1;
  if(props.uses.some(u=>u.material===index&&u.unit===unit))throw Error('This requested unit already has a planned placement.');
  emit('place',model.PhysicalMaterialUnit.create({material:index,unit}));placing.value=undefined;
}catch(e){error.value=String(e);}}
</script>
<template>
<details class="material-demand" @toggle="opened=($event.target as HTMLDetailsElement).open">
<summary>Required materials and receiving ({{materials.length}} lines)</summary>
<template v-if="opened">
<p>Requested quantities stay in this plan. They create no inventory, stock promise, order or payment. Select an existing actual resource or record a received device; receipt does not install it.</p>
<p v-if="editable">Material edits and allocations remain unsaved until you save the installation plan.</p>
<p v-if="error" role="alert">{{error}}</p>
<fieldset :disabled="busy||loading">
<button v-if="editable&&!editing" type="button" :disabled="materials.length>=64" @click="start()">Add material requirement</button>
<ol :start="page*25+1"><li v-for="(material,offset) in rows" :key="page*25+offset">
<strong>{{material.name}}</strong> · {{K[material.kind]?.toLowerCase().replaceAll('_',' ')}} · {{material.allocations.length}} allocated of {{material.quantity}} requested
<dl><template v-for="f in material.specifications" :key="f.name"><dt>{{f.name.replaceAll('_',' ')}}</dt><dd>{{input(f)}}</dd></template></dl>
<ul><li v-for="a in material.allocations" :key="a.unit">Unit {{a.unit+1}}: <template v-if="resource(a.item)">{{resource(a.item)}}</template><template v-else>Resource name unavailable</template> · {{a.basis}}</li></ul>
<button v-if="editable" type="button" @click="start(page*25+offset)">Edit requirement {{page*25+offset+1}}</button>
<button v-if="editable" type="button" @click="emit('remove',page*25+offset)">Remove requirement {{page*25+offset+1}}</button>
<button v-if="editable&&canPlace&&material.kind===K.DEVICE" type="button" @click="placing=page*25+offset;placementUnit='';editing=false;receiving=undefined">Place a unit from requirement {{page*25+offset+1}}</button>
<button v-if="receivable" type="button" @click="startReceiving(page*25+offset)">Receive or reuse a unit for requirement {{page*25+offset+1}}</button>
</li></ol>
<div><button type="button" :disabled="page===0" @click="page--">Previous materials</button><button type="button" :disabled="(page+1)*25>=materials.length" @click="page++">Next materials</button></div>
<form v-if="placing!==undefined&&editable" aria-label="Place requested material" @submit.prevent="place">
<label>Unit to place<input v-model="placementUnit" required inputmode="numeric"/></label>
<button type="submit">Select requested unit for placement</button><button type="button" @click="placing=undefined">Close requested placement</button>
</form>
<form v-if="editing&&editable" aria-label="Material requirement editor" @submit.prevent="commit">
<label>Material requirement name<input v-model="draft.name" required maxlength="160"/></label>
<label>Required quantity<input v-model="quantity" type="text" inputmode="numeric" required/></label>
<label>Required resource kind<select v-model="draft.kind"><option v-for="kind in kinds" :key="kind" :value="kind">{{K[kind].toLowerCase().replaceAll('_',' ')}}</option></select></label>
<details v-if="draft.kind===K.DEVICE||draft.kind===K.PHYSICAL_COMPONENT"><summary>Specify equipment with sources</summary>
<label>Specification field<select v-model="field"><option v-for="f in equipmentFields" :key="f" :value="f">{{f.replaceAll('_',' ')}}</option></select></label>
<EquipmentField :key="field+String(loading)" :name="field" :value="draft.specifications.find(f=>f.name===field)" claims-only @change="claim" @valid="valid=$event"/>
</details>
<details><summary>Reuse an explicit physical specification</summary><PlacementSearch kind="material" :roots="working.roots" @select="source=$event"/><button type="button" :disabled="!source" @click="copySpecification">Use selected resource as specification</button><p v-if="draft.physical">Specification basis: {{draft.physical.basis}}. Occupancy and source completeness were not copied.</p><button v-if="draft.physical" type="button" @click="draft.physical=undefined">Remove physical specification</button></details>
<button type="submit">Apply material requirement</button><button type="button" @click="editing=false">Close requirement editor</button>
</form>
<form v-if="receiving!==undefined&&receivable" aria-label="Material receiving" @submit.prevent="receive(receiving)">
<label>Requested unit number<input v-model="unit" type="text" inputmode="numeric" required/></label>
<label v-if="working.roots.length"><input v-model="wider" type="checkbox"/>Search permitted inventory outside this selection</label>
<PlacementSearch kind="material" :roots="wider?[]:working.roots" :allowed-kinds="[materials[receiving]!.kind]" @select="source=$event"/>
<button v-if="materials[receiving]?.kind===K.DEVICE" type="button" @click="createReceived">Create received device</button>
<label>Receipt or reuse basis<input v-model="basis" required maxlength="512"/></label>
<button type="submit" :disabled="!source">Record actual material allocation</button><button type="button" @click="receiving=undefined">Close receiving</button>
</form>
</fieldset>
</template>
</details>
</template>
<style scoped>
.material-demand{margin:1rem 0;padding:.75rem;border:1px solid var(--line)}
li{padding:.5rem 0}dl{display:grid;grid-template-columns:auto 1fr;gap:.25rem .75rem}dd{margin:0;overflow-wrap:anywhere}form{margin-top:1rem}fieldset{min-width:0}
</style>
