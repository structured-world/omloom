<script setup lang="ts">
import { ref,onMounted } from 'vue';
import { EquipmentClaim, EquipmentReference, EquipmentAlternatives, QuantityRange, type Field } from '../src/generated/omloom/v1/contracts';
const props=defineProps<{ name:string; value?:Field }>();
const title=props.name.replaceAll('_',' ');
const emit=defineEmits<{change:[value:Field|undefined];valid:[value:boolean]}>();
const current=props.value?.value;
const claim=current?.oneofKind==='equipment' ? current.equipment : undefined;
const a=claim?.assertion;
const mode=ref(a?.oneofKind ?? (current?.oneofKind==='clear'?'clear':current?.oneofKind==='text'||current?.oneofKind==='integer' ? 'known':'unknown'));
const known=ref(current?.oneofKind==='text' ? current.text : current?.oneofKind==='integer' ? String(current.integer):'');
const basis=ref(claim?.basis ?? '');
const q=a?.oneofKind==='quantity'?a.quantity:undefined;
function decimal(n:bigint|undefined,scale:number){if(n===undefined)return '';const s=n.toString().padStart(scale+1,'0');return scale?s.slice(0,-scale)+'.'+s.slice(-scale):s;}
const lower=ref(decimal(q?.lower,q?.scale ?? 0));const upper=ref(decimal(q?.upper,q?.scale ?? 0));
const unit=({drive_bays:'count',height:'mm',width:'mm',depth:'mm',weight:'g',power_rating:'W'} as Record<string,string>)[props.name];
const r=ref(EquipmentReference.create(a?.oneofKind==='model'?a.model:{}));
const alternatives=ref(EquipmentAlternatives.create(a?.oneofKind==='alternatives'?a.alternatives:{}));
const confidence=ref(alternatives.value.candidates.some(c=>c.probabilityPpm!==undefined)?'probability':alternatives.value.candidates.some(c=>c.scorePpm!==undefined)?'score':'none');
const error=ref('');
onMounted(()=>emit('valid',true));
function addCandidate(){if(alternatives.value.candidates.length<16)alternatives.value.candidates.push({value:'',reference:undefined,probabilityPpm:undefined,scorePpm:undefined});}
function addExclusion(){if(alternatives.value.excluded.length<16)alternatives.value.excluded.push({value:'',reason:'',coverage:'',reference:undefined});}
function integer(value:string):bigint|undefined {
  if(value==='')return undefined;
  if(!/^\d+$/.test(value))throw Error('Enter nonnegative whole units; use decimal places for scaled measurements.');
  return BigInt(value);
}
function quantity(){
  const values=[lower.value,upper.value];
  if(values.every(v=>v==='')||values.some(v=>v!==''&&!/^\d+(\.\d{1,6})?$/.test(v)))throw Error('Enter at least one known bound, with at most six decimal places.');
  const scale=Math.max(...values.map(v=>v.includes('.')?v.length-v.indexOf('.')-1:0));
  if(unit==='count'&&scale)throw Error('Bays are counted in whole units.');
  const fixed=(s:string)=>s===''?undefined:BigInt((s.includes('.')?s:s+'.').replace('.','').padEnd((s.split('.')[0]?.length ?? 0)+scale,'0'));
  return QuantityRange.create({lower:fixed(lower.value),upper:fixed(upper.value),unit,scale});
}
function numeric(event:Event,set:(value:number|undefined)=>void,places:number){
  const value=(event.target as HTMLInputElement).value;
  if(value===''){set(undefined);changed();return;}
  if(!new RegExp(`^\\d+(\\.\\d{1,${places}})?$`).test(value)){error.value='Enter a nonnegative decimal value.';emit('valid',false);return;}
  const [whole,fraction='']=value.split('.');set(Number(whole+fraction.padEnd(places,'0')));changed();
}
function validateValueBytes(value:EquipmentAlternatives){
  const encoder=new TextEncoder();const scratch=new Uint8Array(160);
  function check(text:string){if(encoder.encodeInto(text,scratch).read!==text.length)throw Error('Enter at most 160 UTF-8 bytes for each possible or excluded value.');}
  for(const candidate of value.candidates)check(candidate.value);
  for(const exclusion of value.excluded)check(exclusion.value);
}
function changed(){
  error.value='';
  emit('valid',true);
  try {
    if(mode.value==='unknown'){emit('change',undefined);return;}
    if(mode.value==='clear'){emit('change',{name:props.name,value:{oneofKind:'clear',clear:0}});return;}
    if(mode.value==='known'){
      if(known.value.trim()==='')throw Error('Enter a known value or leave it unknown.');
      if(props.name==='drive_bays'){
        const count=integer(known.value);if(count===undefined)throw Error('Enter the actual count.');
        emit('change',{name:props.name,value:{oneofKind:'integer',integer:count}});
      }else emit('change',{name:props.name,value:{oneofKind:'text',text:known.value}});
      return;
    }
    let assertion:EquipmentClaim['assertion'];
    if(mode.value==='quantity')assertion={oneofKind:'quantity',quantity:quantity()};
    else if(mode.value==='model')assertion={oneofKind:'model',model:r.value};
    else {
      validateValueBytes(alternatives.value);
      const set=EquipmentAlternatives.clone(alternatives.value);
      for(const c of set.candidates){if(confidence.value!=='probability')c.probabilityPpm=undefined;if(confidence.value!=='score')c.scorePpm=undefined;}
      if(confidence.value!=='probability'){set.calibrationBasis=undefined;set.residualPpm=undefined;}
      assertion={oneofKind:'alternatives',alternatives:set};
    }
    emit('change',{name:props.name,value:{oneofKind:'equipment',equipment:EquipmentClaim.create({basis:basis.value,assertion})}});
  }catch(e){error.value=String(e);emit('valid',false);}
}
</script>
<template>
<fieldset class="equipment-field" @input="changed" @change="changed">
<legend>{{ title }}</legend>
<label>Knowledge for {{ title }}<select v-model="mode" :aria-label="`Knowledge for ${title}`">
<option value="unknown">Preserve / leave unknown</option><option v-if="!unit||name==='drive_bays'" value="known">Known value</option><option v-if="unit" value="quantity">Quantity or range</option><option v-if="!unit" value="alternatives">Possible / excluded values</option><option v-if="name==='model'||name==='variant'" value="model">Reviewed model reference</option><option v-if="value" value="clear">Clear explicitly</option>
</select></label>
<label v-if="mode==='known'">Known {{ title }}<input v-model="known" :maxlength="name==='model'?512:160" :required="mode==='known'" /></label>
<template v-if="!['known','unknown','clear'].includes(mode)">
<label>Evidence basis for {{ title }}<input v-model="basis" maxlength="512" required /></label>
<template v-if="mode==='quantity'"><p>{{ unit }}; an empty bound is unknown. This records a quantity, never installed components.</p>
<label>Minimum {{ title }}<input v-model="lower" inputmode="decimal" /></label><label>Maximum {{ title }}<input v-model="upper" inputmode="decimal" /></label>
</template>
<template v-if="mode==='model'">
<label>Model issuer<input v-model="r.issuer" required maxlength="160" /></label><label>Model key<input v-model="r.key" required maxlength="160" /></label><label>Model basis<input v-model="r.basis" required maxlength="160" /></label>
<p>A reviewed reference preserves this device. It does not install components, acquire catalog data or grant access.</p>
</template>
<template v-if="mode==='alternatives'">
<p>Possible values concern this axis only. Missing from a top-k list does not mean excluded.</p>
<label>Confidence for {{ title }}<select v-model="confidence" :aria-label="`Confidence for ${title}`"><option value="none">No numeric confidence</option><option value="score">Uncalibrated score</option><option value="probability">Attributed probability</option></select></label>
<label>Hypothesis domain for {{ title }}<input v-model="alternatives.domain" maxlength="256" :required="confidence!=='none'" /></label>
<label><input v-model="alternatives.exhaustive" type="checkbox" /> Complete alternative domain for {{ title }}</label>
<div v-for="(c,i) in alternatives.candidates" :key="i" class="candidate">
<label>Possible {{ title }} {{ i+1 }}<input v-model="c.value" required maxlength="160" /></label>
<label v-if="confidence==='probability'">Probability percent {{ i+1 }}<input :value="c.probabilityPpm===undefined?'':c.probabilityPpm/10000" type="number" min="0" max="100" step="0.0001" required @input.stop="numeric($event,v=>c.probabilityPpm=v,4)" /></label>
<label v-if="confidence==='score'">Uncalibrated score {{ i+1 }}<input :value="c.scorePpm===undefined?'':c.scorePpm/1000000" type="number" min="0" max="1" step="0.000001" required @input.stop="numeric($event,v=>c.scorePpm=v,6)" /></label>
<button type="button" @click="alternatives.candidates.splice(i,1);changed()">Remove candidate {{ i+1 }}</button>
<button v-if="!c.reference" type="button" @click="c.reference=EquipmentReference.create();changed()">Add source reference to candidate {{ i+1 }}</button>
<details v-if="c.reference"><summary>Candidate source reference</summary><label>Candidate issuer {{ i+1 }}<input v-model="c.reference.issuer" maxlength="160" required /></label><label>Candidate key {{ i+1 }}<input v-model="c.reference.key" maxlength="160" required /></label><label>Candidate basis {{ i+1 }}<input v-model="c.reference.basis" maxlength="160" required /></label><button type="button" @click="c.reference=undefined;changed()">Remove source reference from candidate {{ i+1 }}</button></details>
</div>
<button type="button" :disabled="alternatives.candidates.length>=16" @click="addCandidate();changed()">Add possible {{ title }}</button>
<template v-if="confidence==='probability'"><label>Calibration basis for {{ title }}<input v-model="alternatives.calibrationBasis" maxlength="256" required /></label><label>Unknown probability percent for {{ title }}<input :value="alternatives.residualPpm===undefined?'':alternatives.residualPpm/10000" type="number" min="0" max="100" step="0.0001" required @input.stop="numeric($event,v=>alternatives.residualPpm=v,4)" /></label><p>Probabilities plus unknown mass must total 100%. A recorded calibration basis is attributed evidence, not certification or permission.</p></template>
<div v-for="(e,i) in alternatives.excluded" :key="i" class="candidate">
<label>Excluded {{ title }} {{ i+1 }}<input v-model="e.value" maxlength="160" required /></label><label>Exclusion reason {{ i+1 }}<input v-model="e.reason" maxlength="512" required /></label><label>Exclusion coverage {{ i+1 }}<input v-model="e.coverage" maxlength="256" required /></label><button type="button" @click="alternatives.excluded.splice(i,1);changed()">Remove exclusion {{ i+1 }}</button>
<button v-if="!e.reference" type="button" @click="e.reference=EquipmentReference.create();changed()">Add source reference to exclusion {{ i+1 }}</button>
<details v-if="e.reference"><summary>Excluded source reference</summary><label>Excluded issuer {{ i+1 }}<input v-model="e.reference.issuer" maxlength="160" required /></label><label>Excluded key {{ i+1 }}<input v-model="e.reference.key" maxlength="160" required /></label><label>Excluded basis {{ i+1 }}<input v-model="e.reference.basis" maxlength="160" required /></label><button type="button" @click="e.reference=undefined;changed()">Remove source reference from exclusion {{ i+1 }}</button></details>
</div>
<button type="button" :disabled="alternatives.excluded.length>=16" @click="addExclusion();changed()">Add excluded {{ title }}</button>
</template>
</template><p v-if="error" role="alert">{{ error }}</p>
</fieldset>
</template>
<style scoped>
.equipment-field{margin-block:1rem;min-width:0}.candidate{border-inline-start:3px solid #cbd6d9;padding:.7rem;margin-block:.5rem}.candidate>button{margin-block:.35rem;margin-inline-end:.35rem}input,select{max-width:100%;box-sizing:border-box}p{overflow-wrap:anywhere}
</style>
