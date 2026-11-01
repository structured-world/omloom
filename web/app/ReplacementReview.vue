<script setup lang="ts">
import {ref,watch} from 'vue';
import {model} from './api';
const props=defineProps<{hypothesis?:model.PhysicalReplacementHypothesis;feedback?:model.PhysicalReplacementFeedback;disabled:boolean;saved:boolean}>();
const emit=defineEmits<{inspect:[];judge:[decision:model.PhysicalReplacementDecision,reason:model.PhysicalReplacementReason,basis:string]}>();
const reason=ref(model.PhysicalReplacementReason.UNSPECIFIED);
const reasons=[model.PhysicalReplacementReason.UNSPECIFIED,model.PhysicalReplacementReason.MOVED,model.PhysicalReplacementReason.ADDITIONAL,model.PhysicalReplacementReason.RESEATED,model.PhysicalReplacementReason.SOURCE_ERROR,model.PhysicalReplacementReason.WRONG_PREDECESSOR];
const strengths:Record<model.PhysicalReplacementStrength,string>={
  [model.PhysicalReplacementStrength.UNSPECIFIED]:'Evidence strength unavailable',
  [model.PhysicalReplacementStrength.LIMITED]:'Limited evidence',
  [model.PhysicalReplacementStrength.CONTEXTUAL]:'Contextual evidence',
  [model.PhysicalReplacementStrength.ESTABLISHED_IDENTITY]:'Established same-part identity',
};
const screen=ref<'evidence'|'decision'|'result'>('evidence');
watch([()=>props.feedback===undefined?undefined:`${props.feedback.decision}/${props.feedback.reason}/${props.feedback.basis}`,()=>props.hypothesis!==undefined],([value,evidence],previous)=>{
  if(value!==previous?.[0]||!evidence)screen.value=value===undefined?'evidence':'result';
},{immediate:true});
</script>
<template><details class="replacement-review" :data-screen="screen"><summary>Replacement evidence and event decision</summary>
<section class="evidence-screen">
<p>Event confidence, endpoint mapping, compatibility and verified service are independent. A decision here neither records installation nor changes wiring.</p>
<button type="button" :disabled="disabled||!saved" @click="emit('inspect')">Explain current replacement hypothesis</button>
<p v-if="!saved">Save the draft before recording a review decision.</p>
<template v-if="hypothesis"><h4>{{strengths[hypothesis.strength]}}</h4><p>{{hypothesis.explanation}}</p><small>Rule {{hypothesis.rule}}. No calibrated probability is available.</small>
<button class="phone-navigation" type="button" @click="screen='decision'">Continue to event decision</button></template></section>
<section v-if="hypothesis" class="decision-screen"><button class="phone-navigation" type="button" @click="screen='evidence'">Back to replacement evidence</button>
<label>Event decision reason<select v-model="reason"><option v-for="value in reasons" :key="value" :value="value">{{value===model.PhysicalReplacementReason.UNSPECIFIED?'No additional reason':model.PhysicalReplacementReason[value]?.toLowerCase().replaceAll('_',' ')}}</option></select></label>
<div class="review-actions"><button :disabled="disabled" type="button" @click="emit('judge',model.PhysicalReplacementDecision.CONFIRMED,reason,hypothesis.basis)">Confirm replacement event</button><button :disabled="disabled" type="button" @click="emit('judge',model.PhysicalReplacementDecision.REJECTED,reason,hypothesis.basis)">Not a replacement</button><button :disabled="disabled" type="button" @click="emit('judge',model.PhysicalReplacementDecision.DEFERRED,reason,hypothesis.basis)">Not sure / later</button></div>
</section>
<section v-if="feedback" class="result-screen"><p>Current attributed judgment: <strong>{{model.PhysicalReplacementDecision[feedback.decision]?.toLowerCase()}}</strong>. Correction window ends {{new Date(Number(feedback.untilMs)).toLocaleString()}}. {{hypothesis&&hypothesis.basis!==feedback.basis?'Evidence has changed since this judgment.':''}}</p><button class="phone-navigation" type="button" @click="screen=hypothesis?'decision':'evidence'">Review event judgment</button></section>
</details></template>
<style scoped>.replacement-review{min-width:0;margin-block:1rem}.replacement-review label{display:grid;gap:.4rem}.replacement-review button,.replacement-review select{min-height:44px;max-width:100%}.review-actions{display:flex;gap:.5rem;flex-wrap:wrap}.phone-navigation{display:none}@media(max-width:700px){.replacement-review button{width:100%}.phone-navigation{display:block}.replacement-review[data-screen="evidence"] .decision-screen,.replacement-review[data-screen="evidence"] .result-screen,.replacement-review[data-screen="decision"] .evidence-screen,.replacement-review[data-screen="decision"] .result-screen,.replacement-review[data-screen="result"] .evidence-screen,.replacement-review[data-screen="result"] .decision-screen{display:none}}</style>
