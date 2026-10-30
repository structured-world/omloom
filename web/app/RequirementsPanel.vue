<script setup lang="ts">
import {ref,computed} from 'vue';
import {call,model,enumLabel,AssetKind,type AssetRef} from './api';
import {equipmentFields} from './equipment-fields';
import type {Field} from '../src/generated/omloom/v1/contracts';
const props=defineProps<{target:AssetRef;kind:AssetKind;working:model.WorkingContext}>();
const field=ref('');const value=ref('');const comparison=ref(model.RequirementComparison.EQUAL);
const layer=ref(model.RequirementLayer.ACCEPTED);const busy=ref(false);const error=ref('');
const result=ref<model.RequirementEvaluation>();
const units:Record<string,string>={drive_bays:'count',height:'mm',width:'mm',depth:'mm',weight:'g',power_rating:'W'};
const numeric=computed(()=>field.value in units);
const fields=computed(()=>props.kind===AssetKind.PROVIDER_ACCOUNT?['delegation']:equipmentFields);
const stateTitle=computed(()=>result.value?.state===model.RequirementState.SATISFIED?'Recorded evidence matches':result.value?.state===model.RequirementState.FAILED?'Requirement not satisfied':'Requirement not established');
function changed(){
  result.value=undefined;error.value='';
  if(!numeric.value)comparison.value=model.RequirementComparison.EQUAL;
  else if(field.value!=='drive_bays'&&comparison.value===model.RequirementComparison.EQUAL)comparison.value=model.RequirementComparison.UNSPECIFIED;
}
async function check(){
  error.value='';result.value=undefined;
  try{
    let expected:Field['value'];
    if(numeric.value){
      if(!/^\d+(\.\d{1,6})?$/.test(value.value))throw Error('Enter a nonnegative measurement with at most six decimal places.');
      const scale=value.value.includes('.')?value.value.length-value.value.indexOf('.')-1:0;
      if(field.value==='drive_bays'&&scale)throw Error('Enter a whole number of bays.');
      const fixed=BigInt(value.value.replace('.',''));
      if(fixed>9223372036854775807n)throw Error('The measurement exceeds the supported integer range.');
      if(field.value==='drive_bays')expected={oneofKind:'integer',integer:fixed};
      else {
        if(comparison.value===model.RequirementComparison.EQUAL)throw Error('Select at least or at most for a measurement.');
        expected={oneofKind:'equipment',equipment:{basis:'Requested constraint',assertion:{oneofKind:'quantity',quantity:{lower:fixed,upper:fixed,unit:units[field.value]!,scale}}}};
      }
    }else expected={oneofKind:'text',text:value.value};
    busy.value=true;
    const response=await call({oneofKind:'evaluateRequirements',evaluateRequirements:model.EvaluateRequirements.create({anchor:props.target,root:0,nodes:[{expression:{oneofKind:'field',field:{target:props.target,expected:{name:field.value,value:expected},comparison:comparison.value,layer:layer.value}}}]})},props.working);
    if(response.oneofKind!=='requirementEvaluation')throw Error('Requirement result unavailable.');
    result.value=response.requirementEvaluation;
  }catch(e){error.value=String(e);}finally{busy.value=false;}
}
</script>
<template>
<details class="requirements-panel">
<summary>Check recorded requirements</summary>
<p>Inspect current evidence for this resource. Matching conditions do not authorize an operation, verify a live path or certify physical fit.</p>
<form @submit.prevent="check" @input="changed" @change="changed">
<fieldset :disabled="busy">
<label>Required fact<select v-model="field" required><option disabled value="">Choose a fact</option><option v-for="name in fields" :key="name" :value="name">{{name.replaceAll('_',' ')}}</option></select></label>
<label>Evidence layer<select v-model="layer"><option :value="model.RequirementLayer.ACCEPTED">Accepted configuration</option><option :value="model.RequirementLayer.OBSERVED">Dated observations</option></select></label>
<label>Required comparison<select v-model="comparison"><option disabled :value="model.RequirementComparison.UNSPECIFIED">Choose a comparison</option><option v-if="!numeric||field==='drive_bays'" :value="model.RequirementComparison.EQUAL">Equals</option><option v-if="numeric" :value="model.RequirementComparison.MINIMUM">At least</option><option v-if="numeric" :value="model.RequirementComparison.MAXIMUM">At most</option></select></label>
<label>Required value<span v-if="numeric"> ({{units[field]}})</span><select v-if="field==='delegation'" v-model="value" required><option disabled value="">Select capability</option><option :value="'ACCOUNT_DELEGATION_SCOPED'">Scoped delegation</option><option :value="'ACCOUNT_DELEGATION_OWNER_ONLY'">Owner action</option></select><input v-else v-model="value" :inputmode="numeric?'decimal':'text'" maxlength="160" required /></label>
<button type="submit" :disabled="!field||!value||comparison===model.RequirementComparison.UNSPECIFIED">{{busy?'Checking…':'Check requirement'}}</button>
</fieldset>
</form>
<p v-if="error" role="alert">{{error}}</p>
<section v-if="result" aria-live="polite" aria-label="Requirement result">
<h3>{{stateTitle}}</h3>
<p>{{enumLabel(model.RequirementReason,result.reason).replaceAll('_',' ')}}</p>
<p>Inventory revision {{result.revision.toString()}}. Evaluated {{new Date(Number(result.evaluatedAtMs)).toLocaleString()}}. This result is not saved.</p>
<template v-for="entry in result.results" :key="entry.semanticKey">
<p v-if="entry.resolutions.includes(model.RequirementResolution.RECORD_FACT)">An authorized inventory edit can supply or correct this fact. Recheck after saving.</p>
<p v-if="entry.resolutions.includes(model.RequirementResolution.OWNER_ACTION)">Use Request owner action on this account's card to review the exact target and change. This check has not submitted a request or granted account access.</p>
<ul v-if="entry.evidence.length"><li v-for="evidence in entry.evidence" :key="evidence.source">{{evidence.observedAtMs===undefined?'Accepted authority':'Observation source'}}: <span class="mono">{{evidence.source}}</span><template v-if="evidence.observedAtMs!==undefined"> · {{new Date(Number(evidence.observedAtMs)).toLocaleString()}}</template><template v-if="evidence.validUntilMs!==undefined"> · valid until {{new Date(Number(evidence.validUntilMs)).toLocaleString()}}</template></li></ul>
</template>
</section>
</details>
</template>
<style scoped>
.requirements-panel{margin-block:1rem;min-width:0}.requirements-panel fieldset{display:grid;gap:.75rem;border:0;padding:.75rem 0}.requirements-panel input,.requirements-panel select{width:100%;max-width:100%;box-sizing:border-box}.requirements-panel li{overflow-wrap:anywhere}.requirements-panel button{min-height:44px}
</style>
