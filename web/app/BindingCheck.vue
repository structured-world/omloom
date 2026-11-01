<script setup lang="ts">
import {ref} from 'vue';
import {call,model,RelationKind,enumLabel,type AssetRef,type Relation} from './api';
const props=defineProps<{target:AssetRef;working:model.WorkingContext}>();
const emit=defineEmits<{configure:[check:model.EvaluateRequirements]}>();
const bindings=ref<Relation[]>(),selected=ref(''),busy=ref(false),error=ref('');
async function load(){if(busy.value)return;busy.value=true;error.value='';try{
  const result=await call({oneofKind:'read',read:model.Read.create({target:props.target})},props.working);
  if(result.oneofKind!=='detail')throw Error('Current bindings are unavailable.');
  bindings.value=result.detail.relations;
}catch(e){error.value=String(e);}finally{busy.value=false;}}
function configure(){const relation=bindings.value?.find(r=>r.relationId===selected.value);if(!relation)return;
  emit('configure',model.EvaluateRequirements.create({anchor:props.target,nodes:[{expression:{oneofKind:'binding',binding:{relationId:relation.relationId,revision:relation.revision,from:relation.from,to:relation.to,kind:relation.kind}}}],root:0}));
}
</script>
<template><details><summary>Check an exact physical or logical binding</summary><p>Require a selected accepted relation and revision to remain current. Removal, recreation or reattachment invalidates this basis. This is inventory evidence, not a signal, ping or service-health test.</p>
<button type="button" :disabled="busy" @click="load">Load permitted current bindings</button>
<template v-if="bindings"><label>Required current binding<select v-model="selected"><option value="">Choose a current binding</option><option v-for="(binding,index) in bindings" :key="binding.relationId" :value="binding.relationId">{{index+1}} · {{enumLabel(RelationKind,binding.kind)}} · revision {{binding.revision.toString()}}</option></select></label><p v-if="!bindings.length">No permitted current bindings.</p><button type="button" :disabled="!selected" @click="configure">Use binding as postcondition</button></template><p v-if="error" role="alert">{{error}}</p></details></template>
