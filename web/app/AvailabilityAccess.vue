<script setup lang="ts">
import {ref} from 'vue';
import {call,model,requestMessage} from './api';
import {millimetres} from './physical-units';
const id=ref(''),busy=ref(false),error=ref(''),view=ref<model.PhysicalView>();
async function read(){if(busy.value)return;busy.value=true;error.value='';view.value=undefined;try{
  const result=await call({oneofKind:'physical',physical:{action:{oneofKind:'availability',availability:{id:id.value}}}},model.WorkingContext.create());
  if(result.oneofKind!=='physical')throw Error('Availability is unavailable.');view.value=result.physical;
}catch(e){error.value=requestMessage(e);}finally{busy.value=false;}}
</script>
<template><details class="availability-access"><summary>Inspect offered capacity</summary>
<p>A profile identifies an explicitly shared region. Its ID grants no access. Availability is advisory; reservation always checks the current state.</p>
<form @submit.prevent="read"><label>Availability profile ID<input v-model="id" required maxlength="36"/></label><button :disabled="busy">Check offered region</button></form>
<p v-if="busy" role="status">Checking the selected region…</p><p v-if="error" role="alert">{{error}}</p>
<section v-if="view?.offers[0]" aria-label="Offered availability"><h3>{{view.offers[0].name}}</h3>
<p role="status">{{view.state===model.RequirementState.SATISFIED?'Available within supplied constraints':view.state===model.RequirementState.FAILED?'Not offered':'Unknown or stale availability'}}</p>
<dl><template v-for="axis in ['x','y','z'] as const" :key="axis"><template v-if="view.offers[0].region?.[axis]"><dt>{{axis.toUpperCase()}} region (mm)</dt><dd>{{millimetres(view.offers[0].region![axis]!.startUm)}} + {{millimetres(view.offers[0].region![axis]!.lengthUm)}}</dd></template></template><dt>Basis revision</dt><dd>{{view.revision.toString()}}</dd></dl>
<p>No occupants, connection paths, identities or counts are supplied by this view.</p>
</section></details></template>
<style scoped>.availability-access{margin-block:1rem;padding:1rem;border:1px solid #bacdd0;border-radius:6px;overflow-wrap:anywhere}form{display:flex;flex-wrap:wrap;gap:.75rem;align-items:end}label{display:grid;gap:.3rem;min-width:0}input,button{box-sizing:border-box;min-height:44px;max-width:100%;font:inherit}input{width:26rem}@media(max-width:600px){form,label,button{width:100%}}</style>
