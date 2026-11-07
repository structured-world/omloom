<script setup lang="ts">
import {ref,computed,watch,onBeforeUnmount,nextTick} from 'vue';
import {call,model,enumLabel,key,AssetKind,type AssetRef} from './api';
import AssetEditor from './AssetEditor.vue';
import RequirementQuestion from './RequirementQuestion.vue';
const props=defineProps<{target:AssetRef;kind:AssetKind;working:model.WorkingContext;plan?:boolean;revision?:bigint;refreshing?:boolean;view?:model.View;refresh?:()=>Promise<void>}>();
const emit=defineEmits<{configure:[query:model.EvaluateRequirements];review:[conditions:model.FieldRequirement[]]}>();
const busy=ref(false);const error=ref('');
const result=ref<model.RequirementEvaluation>();
const question=ref<model.EvaluateRequirements>();
const refinement=ref<{item:model.Item;field:string;question:string}>();
const saveNotice=ref('');
const resultHeading=ref<HTMLElement>();const checkButton=ref<InstanceType<typeof RequirementQuestion>>();
let generation=0;let disposed=false;let returnFocus:HTMLElement|undefined;
onBeforeUnmount(()=>{disposed=true;generation++;});
const contextKey=computed(()=>model.WorkingContext.toJsonString(model.WorkingContext.create({...props.working,expected:undefined})));
function invalidate(){generation++;result.value=undefined;error.value='';busy.value=false;}
watch(()=>props.revision,invalidate);
// Observation values and freshness can change without an accepted-state revision.
watch(()=>props.refreshing,refreshing=>{if(refreshing)invalidate();});
watch(contextKey,()=>{invalidate();question.value=undefined;refinement.value=undefined;});
const stateTitle=computed(()=>result.value?.state===model.RequirementState.SATISFIED?'Recorded evidence matches':result.value?.state===model.RequirementState.FAILED?'Requirement not satisfied':'Requirement not established');
const questionLabel=ref('');
function reviewFindings(){
  const fields=question.value?.nodes.flatMap(node=>node.expression.oneofKind==='field'?[node.expression.field]:[])??[];
  if(fields.length)emit('review',fields);
}
async function evaluate(query:model.EvaluateRequirements,restoreFocus=false){
  if(props.refreshing)return;
  const turn=++generation;busy.value=true;error.value='';result.value=undefined;
  try{
    const response=await call({oneofKind:'evaluateRequirements',evaluateRequirements:query},model.WorkingContext.create({...props.working,expected:undefined}));
    if(disposed||turn!==generation)return;
    if(response.oneofKind!=='requirementEvaluation')throw Error('Requirement result unavailable.');
    result.value=response.requirementEvaluation;
    if(restoreFocus){await nextTick();resultHeading.value?.focus();}
  }catch(e){if(!disposed&&turn===generation){
    error.value=String(e);
    const code=e instanceof Error&&'code' in e?e.code:undefined;
    if(code==='PERMISSION_DENIED'||code==='NOT_FOUND')refinement.value=undefined;
  }}
  finally{if(!disposed&&turn===generation)busy.value=false;}
}
async function clarify(entry:model.RequirementResult){
  const query=question.value;
  if(props.refreshing||!query||!entry.field||key(entry.target)!==key(props.target)||props.kind!==AssetKind.DEVICE||!entry.resolutions.includes(model.RequirementResolution.RECORD_FACT))return;
  returnFocus=document.activeElement instanceof HTMLElement?document.activeElement:undefined;
  const turn=++generation;busy.value=true;error.value='';saveNotice.value='';
  try{
    const reply=await call({oneofKind:'read',read:model.Read.create({target:entry.target,acceptedField:entry.field})},model.WorkingContext.create({...props.working,expected:undefined}));
    if(disposed||turn!==generation||question.value!==query)return;
    if(reply.oneofKind!=='detail'||!reply.detail.item||key(reply.detail.item.target)!==key(props.target))throw Error('This fact is unavailable.');
    refinement.value={item:reply.detail.item,field:entry.field,question:questionLabel.value};
  }catch(e){if(!disposed&&turn===generation){
    error.value=String(e);
    const code=e instanceof Error&&'code' in e?e.code:undefined;
    // Only explicit availability/deadline failures retain the dated answer, never ambiguous internal errors.
    if(code!=='UNAVAILABLE'&&code!=='DEADLINE_EXCEEDED')result.value=undefined;
  }}
  finally{if(!disposed&&turn===generation)busy.value=false;}
}
async function closeRefinement(){refinement.value=undefined;await nextTick();if(returnFocus?.isConnected)returnFocus.focus();else checkButton.value?.focusCheck();}
async function recheck(saved=false,retainInput=false){
  const query=question.value;const context=contextKey.value;
  if(!retainInput)refinement.value=undefined;
  if(saved)saveNotice.value='Fact saved. The separate check below uses current recorded evidence.';
  if(!query)return;
  if(saved&&props.refresh){
    busy.value=true;
    try{await props.refresh();}catch(e){error.value=String(e);busy.value=false;return;}
  }
  if(!disposed&&question.value===query&&contextKey.value===context)await evaluate(query,true);
}
function changed(){
  invalidate();question.value=undefined;saveNotice.value='';
}
async function check(condition:Omit<model.FieldRequirement,'target'>,label:string){
  if(props.refreshing)return;
  error.value='';result.value=undefined;
  try{
    const query=model.EvaluateRequirements.create({anchor:props.target,root:0,nodes:[{expression:{oneofKind:'field',field:{target:props.target,...condition}}}]});
    questionLabel.value=label;
    if(props.plan){emit('configure',query);return;}
    question.value=query;saveNotice.value='';
    await evaluate(query);
  }catch(e){error.value=String(e);}
}
</script>
<template>
<details class="requirements-panel">
<summary>{{plan?'Configure a postcondition':'Check recorded requirements'}}</summary>
<p>{{plan?'Select the condition evaluated after reported work. Dated samples need qualified post-effect evidence; an old successful sample cannot verify this step.':'Inspect current evidence for this resource. Matching conditions do not authorize an operation, verify a live path or certify physical fit.'}}</p>
<RequirementQuestion ref="checkButton" :kind="kind" :busy="busy" :disabled="refreshing" :plan="plan" @check="check" @change="changed" />
<p v-if="error" role="alert">{{error}}</p>
<p v-if="saveNotice" role="status">{{saveNotice}}</p>
<button v-if="question&&!result&&!busy" type="button" :disabled="refreshing" @click="recheck()">Retry check</button>
<section v-if="result" aria-live="polite" aria-label="Requirement result">
<h3 ref="resultHeading" tabindex="-1">{{stateTitle}}</h3>
<p>{{questionLabel}}</p>
<p>{{enumLabel(model.RequirementReason,result.reason).replaceAll('_',' ')}}</p>
<p>Inventory revision {{result.revision.toString()}}. Evaluated {{new Date(Number(result.evaluatedAtMs)).toLocaleString()}}. This result is not saved.</p>
<button v-if="kind===AssetKind.DEVICE&&result.results.some(entry=>!!entry.target&&!!entry.field)" type="button" :disabled="busy||refreshing" @click="reviewFindings">Review these findings</button>
<template v-for="entry in result.results" :key="entry.semanticKey">
<button v-if="kind===AssetKind.DEVICE&&entry.resolutions.includes(model.RequirementResolution.RECORD_FACT)" type="button" :disabled="busy||refreshing" @click="clarify(entry)">{{entry.reason===model.RequirementReason.MISMATCH?'Correct recorded fact':'Clarify this fact'}}</button>
<p v-if="kind!==AssetKind.DEVICE&&entry.resolutions.includes(model.RequirementResolution.RECORD_FACT)">An authorized inventory edit can supply or correct this fact. Recheck after saving.</p>
<p v-if="entry.reason===model.RequirementReason.MISMATCH&&entry.resolutions.includes(model.RequirementResolution.RECORD_FACT)">A correctly recorded shortage is not fixed by changing the record. Correct it only if the fact is wrong.</p>
<p v-if="entry.resolutions.includes(model.RequirementResolution.OWNER_ACTION)">Use Request owner action on this account's card when it is shown. It requires a visible managed resource. This check has not submitted a request or granted account access.</p>
<ul v-if="entry.evidence.length"><li v-for="evidence in entry.evidence" :key="evidence.source">{{evidence.observedAtMs===undefined?'Accepted authority':'Observation source'}}: <span class="mono">{{evidence.source}}</span><template v-if="evidence.observedAtMs!==undefined"> · {{new Date(Number(evidence.observedAtMs)).toLocaleString()}}</template><template v-if="evidence.validUntilMs!==undefined"> · valid until {{new Date(Number(evidence.validUntilMs)).toLocaleString()}}</template></li></ul>
</template>
</section>
<AssetEditor v-if="refinement" :item="refinement.item" :definitions="[]" :field-mask="[refinement.field]" :question="refinement.question" :working="working" :view="view ?? model.View.OVERVIEW" @close="closeRefinement" @saved="recheck(true)" @recheck="recheck(false,$event)" />
</details>
</template>
