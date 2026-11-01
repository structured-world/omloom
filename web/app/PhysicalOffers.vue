<script setup lang="ts">
import {ref} from 'vue';
import {call,model,requestMessage,type AssetRef} from './api';
import {micrometres,millimetres,signedMicrometres} from './physical-units';
const props=defineProps<{target:AssetRef;working:model.WorkingContext;inactive:boolean}>();
const emit=defineEmits<{changed:[]}>();
const opened=ref(false),busy=ref(false),error=ref(''),notice=ref(''),revision=ref<bigint>();
const offers=ref<model.PhysicalOffer[]>([]),next=ref<string>(),editing=ref<model.PhysicalOffer>();
const name=ref(''),recipient=ref(''),mass=ref(false),power=ref(false),cooling=ref(false),region=ref<model.PhysicalVolume>();
const axes=['x','y','z'] as const;
const starts=ref(['','','']),lengths=ref(['','','']);
async function invoke(action:model.Physical['action']){const result=await call({oneofKind:'physical',physical:{action}},{...props.working,expected:revision.value});if(result.oneofKind!=='physical')throw Error('Availability profiles are unavailable.');revision.value=result.physical.revision;return result.physical;}
async function run(fn:()=>Promise<void>){if(busy.value||props.inactive)return;busy.value=true;error.value='';notice.value='';try{await fn();}catch(e){error.value=requestMessage(e);}finally{busy.value=false;}}
async function load(after?:string){const view=await invoke({oneofKind:'offers',offers:{enclosure:props.target,after}});offers.value=view.offers;next.value=view.next;opened.value=true;}
function edit(offer?:model.PhysicalOffer){editing.value=offer;name.value=offer?.name??'';recipient.value=offer?.recipient??'';mass.value=offer?.mass??false;power.value=offer?.power??false;cooling.value=offer?.cooling??false;region.value=offer?.region===undefined?undefined:model.PhysicalVolume.clone(offer.region);copyInputs();}
function copyInputs(){for(const [i,axis] of axes.entries()){const extent=region.value?.[axis];starts.value[i]=extent?millimetres(extent.startUm):'';lengths.value[i]=extent?millimetres(extent.lengthUm):'';}}
async function useEnvelope(){await run(async()=>{const view=await invoke({oneofKind:'read',read:{enclosure:props.target,occupants:false}});region.value=view.definitions[0]?.volume;copyInputs();if(!region.value)throw Error('Usable enclosure geometry is unknown.');});}
function coordinate(text:string){return signedMicrometres(text);}
async function save(revoke=false){await run(async()=>{
  const region=model.PhysicalVolume.create();for(const [i,axis] of axes.entries())region[axis]={startUm:coordinate(starts.value[i]!),lengthUm:micrometres(lengths.value[i]!)};
  await invoke({oneofKind:'offer',offer:{offer:model.PhysicalOffer.create({...editing.value,enclosure:props.target,name:name.value,recipient:recipient.value,region,mass:mass.value,power:power.value,cooling:cooling.value,revoked:revoke})}});
  edit();await load();notice.value=revoke?'Profile revoked. New availability disclosure is stopped.':'Profile saved. No capacity is reserved.';emit('changed');
});}
</script>
<template><details class="physical-offers" :inert="inactive" @toggle="($event.target as HTMLDetailsElement).open&&!opened&&run(()=>load())"><summary>Offer selected capacity without occupant access</summary>
<p>The recipient can infer availability changes within this exact region. Publishing does not disclose individual equipment, sizes, connections or counts, and grants no reservation or equipment access.</p>
<p v-if="error" role="alert">{{error}}</p><p v-if="notice||busy" role="status">{{busy?'Updating availability profiles…':notice}}</p>
<template v-if="opened"><ul><li v-for="offer in offers" :key="offer.id"><strong>{{offer.name}}</strong> · {{offer.revoked?'revoked':'active'}}<p>Recipient: {{offer.recipient}} · Profile ID: <code>{{offer.id}}</code></p><button :disabled="busy" @click="edit(offer)">Edit offer: {{offer.name}}</button></li></ul><nav><button :disabled="busy" @click="run(()=>load())">First offer page</button><button v-if="next" :disabled="busy" @click="run(()=>load(next))">Next offer page</button></nav>
<form @submit.prevent="save()"><fieldset :disabled="busy"><legend>{{editing?'Edit reviewed disclosure':'Create a region disclosure'}}</legend>
<label>Offer name<input v-model="name" required maxlength="160"/></label><label>Recipient identity<input v-model="recipient" required maxlength="128"/></label>
<button type="button" @click="useEnvelope">Use current usable enclosure region</button>
<div v-for="(axis,index) in axes" :key="axis" class="axis"><label>{{axis.toUpperCase()}} origin (mm)<input v-model="starts[index]" required maxlength="21" inputmode="decimal"/></label><label>{{axis.toUpperCase()}} length (mm)<input v-model="lengths[index]" required maxlength="20" inputmode="decimal"/></label></div>
<label class="check"><input v-model="mass" type="checkbox"/>Disclose the enclosure mass admission result</label><label class="check"><input v-model="power" type="checkbox"/>Disclose the enclosure power admission result</label><label class="check"><input v-model="cooling" type="checkbox"/>Disclose the enclosure cooling admission result</label>
<button>Save reviewed offer</button><button v-if="editing&&!editing.revoked" type="button" @click="save(true)">Revoke this offer</button><button v-if="editing" type="button" @click="edit()">New offer</button>
</fieldset></form></template></details></template>
<style scoped>.physical-offers{margin-block:1rem;min-width:0;overflow-wrap:anywhere}fieldset{display:grid;gap:.65rem;min-width:0;border:0;padding:0}label{display:grid;gap:.3rem}.check{display:flex;align-items:center}.axis{display:grid;grid-template-columns:1fr 1fr;gap:.5rem}input:not([type=checkbox]),button{min-height:44px;box-sizing:border-box;width:100%;font:inherit}nav{display:flex;gap:.5rem}code{user-select:all}form{margin-block:1rem}@media(max-width:600px){.axis{grid-template-columns:1fr}}</style>
