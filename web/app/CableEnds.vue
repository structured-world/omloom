<script setup lang="ts">
import {ref} from 'vue';
import {call,model,key,input,type AssetRef} from './api';
import MatingEditor from './MatingEditor.vue';
const props=defineProps<{target:AssetRef;working:model.WorkingContext;inactive:boolean}>();
const emit=defineEmits<{changed:[]}>();
const view=ref(model.PhysicalView.create()),opened=ref(false),busy=ref(false),error=ref('');
const editing=ref<model.PhysicalCableEnd>(),structural=ref(''),name=ref(''),basis=ref(''),connector=ref('');
const mating=ref<model.PhysicalMating>();
const channels=ref('');
function label(port:AssetRef|undefined){const item=view.value.items.find(i=>key(i.target)===key(port));return item?input(item.fields.find(f=>f.name==='name')):undefined;}
async function invoke(action:model.Physical['action']) {
  const result=await call({oneofKind:'physical',physical:{action}},{...props.working,expected:view.value.revision});
  if(result.oneofKind!=='physical')throw Error('Cable ends are unavailable.');
  view.value.revision=result.physical.revision;
  return result.physical;
}
async function load(after?:string){if(busy.value||props.inactive)return;busy.value=true;error.value='';try{view.value=await invoke({oneofKind:'cableEnds',cableEnds:{cable:props.target,after}});opened.value=true;}catch(e){error.value=String(e);}finally{busy.value=false;}}
function edit(end?:model.PhysicalCableEnd){editing.value=end;structural.value=end?.key??'';name.value=end?.name??'';basis.value=end?.basis??'';connector.value=end?.connector??'';mating.value=end?.mating===undefined?undefined:model.PhysicalMating.clone(end.mating);channels.value=end===undefined?'':end.channels.join(', ');}
async function save(){if(busy.value||props.inactive)return;busy.value=true;error.value='';try{
  const selected=channels.value.trim()===''?[]:channels.value.split(',').map(value=>{const text=value.trim();if(!/^[1-9][0-9]{0,3}$/.test(text)||Number(text)>4096)throw Error('Use distinct channel numbers from 1 to 4096, separated by commas.');return Number(text);});
  if(selected.length>64||new Set(selected).size!==selected.length)throw Error('Use at most 64 distinct channels per end.');
  await invoke({oneofKind:'cableEnd',cableEnd:{cable:props.target,expected:editing.value?.revision,end:model.PhysicalCableEnd.create({key:structural.value,name:name.value,basis:basis.value,connector:connector.value===''?undefined:connector.value,mating:mating.value,channels:selected})}});
  edit();emit('changed');
  try{view.value=await invoke({oneofKind:'cableEnds',cableEnds:{cable:props.target}});}catch(e){error.value='End definition saved. The saved cable ends could not be refreshed: '+String(e);}
}catch(e){error.value=String(e);}finally{busy.value=false;}}
</script>
<template>
<section class="cable-ends" aria-label="Cable ends">
<button :disabled="busy||inactive" @click="load()">Inspect named cable ends</button>
<template v-if="opened">
<p>End identity belongs to this cable. A connector label does not establish an attachment, lane mapping or working link.</p>
<ul><li v-for="end in view.cableEnds" :key="end.key"><strong>{{end.name}}</strong> · {{end.key}}<span v-if="end.connector"> · {{end.connector}}</span><p>Definition source: {{end.basis}}</p><p v-if="end.attachment.oneofKind==='connected'">Reported attached: {{label(end.attachment.connected.port)??'Permitted physical port'}}. Service not verified.</p><p v-else-if="end.attachment.oneofKind==='loose'">Confirmed loose.</p><p v-else>Attachment unknown or unavailable.</p><p v-if="end.intendedPort">Intended destination: {{label(end.intendedPort)??'Permitted physical port'}}.</p><button :disabled="busy||inactive" @click="edit(end)">Edit end: {{end.name}}</button></li></ul>
<p v-if="!view.cableEnds.length">No named ends have been recorded for this cable.</p>
<nav aria-label="Cable-end pages"><button :disabled="busy||inactive" @click="load()">First end page</button><button v-if="view.next" :disabled="busy||inactive" @click="load(view.next)">Next end page</button></nav>
<form @submit.prevent="save">
<fieldset :disabled="busy||inactive">
<h4>{{editing?'Edit current end definition':'Record a named end'}}</h4>
<label>Structural end key<input v-model="structural" required maxlength="64" pattern="[A-Za-z0-9_.:-]+" :disabled="busy||!!editing" /></label>
<label>End label<input v-model="name" required maxlength="128" :disabled="busy" /></label>
<label>End definition source<input v-model="basis" required maxlength="512" :disabled="busy" /></label>
<label>Connector designation, if known<input v-model="connector" maxlength="128" :disabled="busy" /></label>
<label>Known assembly channels<input v-model="channels" maxlength="384" :disabled="busy" placeholder="For example: 1, 2, 3, 4"/></label><p>Equal channel numbers at two ends describe one internal path in this cable. A breakout has several ends in the same assembly. Empty means unknown, not all channels. This does not establish polarity, speed or live service.</p>
<MatingEditor v-model="mating"/>
<button :disabled="busy">Save end definition</button><button v-if="editing" type="button" :disabled="busy" @click="edit()">Start a different end</button>
</fieldset>
</form>
</template>
<p v-if="error" role="alert">{{error}}</p>
</section>
</template>
<style scoped>
.cable-ends{border-block-start:1px solid #c4d2d6;padding-block:1rem}form{max-width:36rem}fieldset{display:grid;gap:.6rem;border:0;padding:0;min-width:0}label{display:grid;gap:.25rem}input,button{font:inherit;min-height:44px}input{box-sizing:border-box;width:100%;min-width:0}li{overflow-wrap:anywhere;margin-block:.7rem}nav{display:flex;gap:.5rem;flex-wrap:wrap}
</style>
