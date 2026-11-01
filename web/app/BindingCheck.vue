<script setup lang="ts">
import {ref,watch} from 'vue';
import {call,model,key,RelationKind,enumLabel,type AssetRef,type Relation} from './api';
const props=defineProps<{target:AssetRef;working:model.WorkingContext}>();
const emit=defineEmits<{configure:[check:model.EvaluateRequirements]}>();
const bindings=ref<Relation[]>(),selected=ref(''),busy=ref(false),error=ref('');
const next=ref<string>();let generation=0;
watch(()=>[key(props.target),props.working.expected],()=>{generation++;bindings.value=undefined;selected.value='';next.value=undefined;busy.value=false;});
async function load(cursor?:string){if(busy.value)return;const turn=++generation;busy.value=true;error.value='';try{
  const result=await call({oneofKind:'read',read:model.Read.create({target:props.target,relationsCursor:cursor})},props.working);
  if(result.oneofKind!=='detail')throw Error('Current bindings are unavailable.');
  if(turn!==generation)return;
  bindings.value=result.detail.relations;selected.value='';next.value=result.detail.relationsNext;
}catch(e){if(turn===generation)error.value=String(e);}finally{if(turn===generation)busy.value=false;}}
function configure(){const relation=bindings.value?.find(r=>r.relationId===selected.value);if(!relation)return;
  emit('configure',model.EvaluateRequirements.create({anchor:props.target,nodes:[{expression:{oneofKind:'binding',binding:{relationId:relation.relationId,revision:relation.revision,from:relation.from,to:relation.to,kind:relation.kind}}}],root:0}));
}
</script>
<template><details><summary>Check an exact physical or logical binding</summary><p>Require a selected accepted relation and revision to remain current. Removal, recreation or reattachment invalidates this basis. This is inventory evidence, not a signal, ping or service-health test.</p>
<button type="button" :disabled="busy" @click="load()">Load permitted current bindings</button>
<button v-if="next" type="button" :disabled="busy" @click="load(next)">Next binding page</button>
<template v-if="bindings"><label>Required current binding<select v-model="selected"><option value="">Choose a current binding</option><option v-for="(binding,index) in bindings" :key="binding.relationId" :value="binding.relationId">{{index+1}} · {{enumLabel(RelationKind,binding.kind)}} · revision {{binding.revision.toString()}}</option></select></label><p v-if="!bindings.length">No permitted current bindings.</p><button type="button" :disabled="!selected" @click="configure">Use binding as postcondition</button></template><p v-if="error" role="alert">{{error}}</p></details></template>
