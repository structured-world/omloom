<script setup lang="ts">
import {computed,ref,shallowRef,watch,nextTick,onBeforeUnmount} from 'vue';
import {call,model,key,identityName,AssetKind as K,type AssetRef} from './api';
import {sceneData} from './physical-scene-data';
import type {PhysicalSceneRenderer} from './physical-scene-renderer';
const props=defineProps<{items:model.Item[];working:model.WorkingContext;focus?:model.Item;inactive:boolean}>();
const emit=defineEmits<{open:[target:AssetRef]}>();
const eligible=(item:model.Item)=>[K.SITE,K.LOCATION,K.RACK,K.DEVICE,K.PORT,K.PHYSICAL_COMPONENT,K.PHYSICAL_POSITION,K.CABLE].includes(item.kind);
const candidates=computed(()=>{const values=props.focus?[props.focus]:props.items;const frame=view.value.items.find(v=>key(v.target)===key(root.value));return [...new Map([...values,...(frame?[frame]:[])].filter(eligible).map(v=>[key(v.target),v])).values()];});
const root=ref<AssetRef>(),subjects=ref<AssetRef[]>([]),selected=ref<AssetRef>();
const view=shallowRef(model.PhysicalView.create()),busy=ref(false),error=ref(''),graphicsError=ref('');
const continuations=ref(new Map<string,string>()),host=ref<HTMLElement>();
let renderer:PhysicalSceneRenderer|undefined,generation=0,disposed=false;
const data=computed(()=>root.value?sceneData(view.value,root.value):{boxes:[],missing:[],lines:[]});
const selectedItem=computed(()=>view.value.items.find(i=>key(i.target)===key(selected.value)));
const selectionDescription=computed(()=>selectedItem.value?.kind===K.CABLE?'Recorded named-end bindings. The diagram is not a surveyed route or live-connectivity check.':data.value.missing.some(i=>key(i.target)===key(selected.value))?'Position or geometry is unknown in this frame.':'Recorded enclosure or placement envelope; no fit or live-service claim.');
function choose(target:AssetRef){selected.value=target;renderer?.highlight(target);}
const sceneWorking=()=>model.WorkingContext.create({roots:root.value?[root.value]:props.working.roots});
async function paint(){
  await nextTick();if(!host.value||disposed)return;
  if(!renderer&&!graphicsError.value){try{const{PhysicalSceneRenderer}=await import('./physical-scene-renderer');if(disposed||!host.value)return;if(!renderer)renderer=new PhysicalSceneRenderer(host.value,choose,message=>{graphicsError.value=message;});}catch(e){graphicsError.value=`3D graphics are unavailable. The resource list remains usable. ${String(e)}`;}}
  renderer?.update(data.value.boxes,data.value.lines);renderer?.highlight(selected.value);
}
async function refresh(){
  if(!subjects.value.length)return;const token=++generation;busy.value=true;error.value='';
  view.value=model.PhysicalView.create();renderer?.update([],[]);
  try{const reply=await call({oneofKind:'physical',physical:model.Physical.create({action:{oneofKind:'scene',scene:{subjects:subjects.value}}})},sceneWorking());if(token!==generation||disposed)return;if(reply.oneofKind!=='physical')throw Error('The scene projection is unavailable.');view.value=reply.physical;await paint();}
  catch(e){if(token===generation)error.value=String(e);}finally{if(token===generation)busy.value=false;}
}
async function expand(target:AssetRef,more=false){
  if(busy.value)return;const token=++generation;busy.value=true;error.value='';
  try{const reply=await call({oneofKind:'query',query:model.Query.create({view:model.View.HARDWARE,roots:root.value?[root.value]:[],focus:target,levelOnly:true,requirePhysicalRead:true,limit:25,columns:['name'],cursor:more?continuations.value.get(key(target)):undefined})});if(token!==generation||disposed)return;if(reply.oneofKind!=='page')throw Error('Components are unavailable.');
    const next=new Map(subjects.value.map(v=>[key(v),v]));for(const item of reply.page.items)if(item.target&&eligible(item))next.set(key(item.target),item.target);
    if(next.size>128)throw Error('The scene supports 128 selected resources. Open a smaller enclosure or return to the table.');
    subjects.value=[...next.values()];if(reply.page.next)continuations.value.set(key(target),reply.page.next);else continuations.value.delete(key(target));
  }catch(e){if(token===generation&&!disposed)error.value=String(e);return;}finally{if(token===generation)busy.value=false;}
  await refresh();
}
async function setRoot(value:AssetRef|undefined){generation++;busy.value=false;error.value='';root.value=value;selected.value=value;subjects.value=value?[value]:[];continuations.value=new Map();view.value=model.PhysicalView.create();renderer?.update([],[]);if(value)await expand(value);}
function retryGraphics(){renderer?.dispose();renderer=undefined;graphicsError.value='';void paint();}
watch(()=>[props.working.roots.map(key).join('|'),key(props.focus?.target),props.inactive]as const,()=>{if(props.inactive){generation++;view.value=model.PhysicalView.create();renderer?.update([],[]);return;}void setRoot(props.focus?(eligible(props.focus)?props.focus.target:undefined):(props.working.roots.length===1?props.working.roots[0]:candidates.value.length===1?candidates.value[0]?.target:undefined));},{immediate:true});
onBeforeUnmount(()=>{disposed=true;generation++;renderer?.dispose();});
</script>
<template><section class="physical-scene" aria-label="Physical 3D workspace">
<header class="scene-heading"><div><h2>Physical workspace</h2><p>Recorded placement envelopes. Connection lines are diagrams, not surveyed cable routes.</p></div><button :disabled="busy||!root" @click="refresh">Refresh scene</button></header>
<label class="scene-root">Enclosure<select aria-label="3D enclosure" :value="key(root)" :disabled="busy" @change="setRoot(candidates.find(v=>key(v.target)===($event.target as HTMLSelectElement).value)?.target)"><option value="">Choose a resource in this selection</option><option v-for="item in candidates" :key="key(item.target)" :value="key(item.target)">{{identityName(item)}}</option></select></label>
<p v-if="busy" role="status">Reading selected current geometry…</p><p v-if="error" role="alert">{{error}}</p>
<p v-if="graphicsError" role="alert">{{graphicsError}} <button @click="retryGraphics">Retry 3D</button></p>
<div class="scene-stage" ref="host"><p v-if="!data.boxes.length&&!busy" class="scene-empty">{{root?'No recorded geometry can be placed in this frame. Inspect the resource list or record physical facts.':'Choose an enclosure to inspect its recorded contents.'}}</p></div>
<div v-if="root" class="scene-camera" aria-label="3D camera"><button @click="renderer?.rotate(-1)">Rotate left</button><button @click="renderer?.reset()">Reset camera</button><button @click="renderer?.rotate(1)">Rotate right</button></div>
<div class="scene-inspection"><section v-if="selectedItem" aria-label="Selected 3D resource"><h3>{{identityName(selectedItem)}}</h3><p>{{selectionDescription}}</p><div class="actions"><button v-if="selected" :disabled="busy" @click="emit('open',selected)">Open resource</button><button v-if="selected&&selectedItem.kind!==K.CABLE" :disabled="busy" @click="expand(selected)">Show components</button><button v-if="selected&&continuations.has(key(selected))" :disabled="busy" @click="expand(selected,true)">More components</button></div></section>
<section aria-label="3D resource list"><h3>Shown resources</h3><ul><li v-for="item in view.items" :key="key(item.target)"><button :aria-pressed="key(item.target)===key(selected)" @click="item.target&&choose(item.target)">{{identityName(item)}}</button><span v-if="data.boxes.some(v=>key(v.target)===key(item.target)&&v.cutaway)">Opened enclosure (cutaway)</span><span v-if="data.missing.some(v=>key(v.target)===key(item.target))">Position unknown</span></li></ul><p v-if="root">{{data.boxes.length}} recorded envelopes · {{data.lines.length}} two-end cable diagrams. Missing or hidden data is not proof of empty space or disconnection.</p></section></div>
</section></template>
