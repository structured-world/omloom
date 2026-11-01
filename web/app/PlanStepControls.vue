<script setup lang="ts">
import {ref,defineAsyncComponent} from 'vue';
import {model,enumLabel,input,type AssetRef} from './api';
import RequirementsPanel from './RequirementsPanel.vue';
import BindingCheck from './BindingCheck.vue';
const PowerPathsPanel=defineAsyncComponent(()=>import('./PowerPathsPanel.vue'));
const props=defineProps<{control?:model.PlanControl;index:number;predecessors:{title:string;hasCheck:boolean}[];subject?:AssetRef;kind?:number;working:model.WorkingContext;editable:boolean;readiness:number;checkedState?:number;checkedReason?:number}>();
const emit=defineEmits<{change:[control:model.PlanControl]}>();
const previous=ref(''),when=ref(model.PlanTrigger.REPORTED),error=ref('');
const readinessTitles:Record<model.PlanReadiness,string>={
  [model.PlanReadiness.UNSPECIFIED]:'Save or check the plan to evaluate prerequisites',
  [model.PlanReadiness.READY]:'Ready to report',
  [model.PlanReadiness.WAITING]:'Waiting for a prerequisite',
  [model.PlanReadiness.UNSELECTED]:'This branch is not selected',
  [model.PlanReadiness.COMPLETE]:'Work reported or resolved',
};
function current(){return props.control?model.PlanControl.create(props.control):model.PlanControl.create();}
function add(){error.value='';if(previous.value==='')return;const step=Number(previous.value);if(!Number.isInteger(step)||step<0||step>=props.index)return;if(when.value!==model.PlanTrigger.REPORTED&&!props.predecessors[step]?.hasCheck){error.value='Configure a postcondition on that preceding step first.';return;}const control=current();if(control.after.some(d=>d.step===step)){error.value='This prerequisite is already configured.';return;}control.after.push({step,when:when.value});emit('change',control);}
function remove(index:number){const control=current();control.after.splice(index,1);emit('change',control);}
function condition(check?:model.EvaluateRequirements){const control=current();control.check=check;emit('change',control);}
</script>
<template><section class="step-controls" :aria-label="`Step ${index+1} controls`">
<p>Step {{index+1}} · {{readinessTitles[readiness as model.PlanReadiness]}}</p>
<p v-if="error" role="alert">{{error}}</p>
<p v-if="!control?.after.length">No prerequisites: this step is independent of list order.</p>
<ul v-if="control?.after.length"><li v-for="(edge,i) in control.after" :key="edge.step">After step {{edge.step+1}}: {{enumLabel(model.PlanTrigger,edge.when).replaceAll('_',' ')}} <button v-if="editable" type="button" @click="remove(i)">Remove prerequisite</button></li></ul>
<fieldset v-if="editable&&index>0"><legend>Step prerequisites</legend>
<label>Preceding step<select v-model="previous"><option value="">Choose a step</option><option v-for="(entry,i) in predecessors" :key="i" :value="String(i)">{{i+1}} · {{entry.title}}</option></select></label>
<label>Continue when<select v-model="when"><option :value="model.PlanTrigger.REPORTED">Work reported</option><option :value="model.PlanTrigger.SATISFIED">Postcondition satisfied</option><option :value="model.PlanTrigger.FAILED">Postcondition failed</option></select></label>
<button type="button" :disabled="previous===''" @click="add">Add prerequisite</button>
</fieldset>
<p v-if="control?.check">Postcondition configured. <span v-if="checkedState!==undefined">Current result: {{enumLabel(model.RequirementState,checkedState).replaceAll('_',' ')}}<template v-if="checkedReason!==undefined"> · {{enumLabel(model.RequirementReason,checkedReason).replaceAll('_',' ')}}</template>.</span><span v-else>Evaluated after this work is reported.</span> <button v-if="editable" type="button" @click="condition()">Remove postcondition</button></p>
<ul v-if="control?.check" aria-label="Configured postcondition"><li v-for="(node,i) in control.check.nodes" :key="i">
<template v-if="node.expression.oneofKind==='field'">{{node.expression.field.expected?.name}} · {{enumLabel(model.RequirementComparison,node.expression.field.comparison).replaceAll('_',' ')}} · {{input(node.expression.field.expected)}} · {{enumLabel(model.RequirementLayer,node.expression.field.layer).replaceAll('_',' ')}}</template>
<template v-else-if="node.expression.oneofKind==='powerPaths'">Recorded power path separation: {{node.expression.powerPaths.sources.length}} inputs, {{node.expression.powerPaths.boundaries.length}} terminal boundaries</template>
<template v-else-if="node.expression.oneofKind==='binding'">Required exact binding revision {{node.expression.binding.revision.toString()}}</template>
<template v-else>Condition group {{i+1}}</template></li></ul>
<RequirementsPanel v-if="editable&&subject&&kind!==undefined" :target="subject" :kind="kind" :working="working" plan @configure="condition"/>
<PowerPathsPanel v-if="editable&&subject" :target="subject" :working="working" plan @configure="condition"/>
<BindingCheck v-if="editable&&subject" :target="subject" :working="working" @configure="condition"/>
</section></template>
<style scoped>
.step-controls{border-left:3px solid #b6c9cb;padding-left:.75rem;margin-block:.75rem;min-width:0}.step-controls fieldset{border:0;padding:0}.step-controls label{display:grid;gap:.35rem;margin-block:.5rem}.step-controls button,.step-controls select{min-height:44px;max-width:100%;box-sizing:border-box}.step-controls li{overflow-wrap:anywhere}@media(max-width:700px){.step-controls button,.step-controls select{width:100%}}
</style>
