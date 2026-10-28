<script setup lang="ts">
import { onMounted, ref, computed } from 'vue';
import Dialog from './Dialog.vue';
import { call, model, display, key, type AssetRef } from './api';
const props = defineProps<{ item:model.Item; working:model.WorkingContext }>();
const emit = defineEmits<{ changed:[]; open:[target:AssetRef] }>();
const state = ref<model.ResponsibilityState>();
const error = ref(''); const busy = ref(false); const editing = ref(false); const importing = ref(false);
const facet = ref(model.ResponsibilityFacet.CLAIMED_OWNER);
const mode = ref(model.AssignmentMode.EXPLICIT);
const party = ref(model.PartyRef.create({kind:model.PartyKind.TEAM})); const contact = ref(''); const source = ref('');
const preview = ref<model.ResponsibilityState>(); const prepared = ref<model.Responsibility>();
const json = ref(''); const imported = ref<model.ResponsibilityTransfer>(); const importPreview = ref<model.ResponsibilityTransferState>();
const facets = [model.ResponsibilityFacet.CLAIMED_OWNER,model.ResponsibilityFacet.OPERATOR,model.ResponsibilityFacet.BENEFICIARY,model.ResponsibilityFacet.ACCOUNT_CONTROLLER];
const names:Record<model.ResponsibilityFacet,string> = { [model.ResponsibilityFacet.UNSPECIFIED]:'Unspecified',[model.ResponsibilityFacet.CLAIMED_OWNER]:'Claimed owner',[model.ResponsibilityFacet.OPERATOR]:'Operator',[model.ResponsibilityFacet.BENEFICIARY]:'Beneficiary',[model.ResponsibilityFacet.ACCOUNT_CONTROLLER]:'Account controller' };
const status = (value:number) => model.ResponsibilityStatus[value]?.toLowerCase().replaceAll('_',' ');
const inheritable = computed(()=>[model.ResponsibilityFacet.CLAIMED_OWNER,model.ResponsibilityFacet.OPERATOR].includes(facet.value));
const sources = computed(()=>state.value?.sources.filter(s=>s.facets.includes(facet.value)) ?? []);
function working() { return model.WorkingContext.create({...props.working, expected:state.value?.revision}); }
async function load(includeSources=false) {
  const result=await call({oneofKind:'responsibility',responsibility:model.Responsibility.create({target:props.item.target,action:model.ResponsibilityAction.READ,includeSources})},working());
  if(result.oneofKind!=='responsibility') throw Error('Unexpected responsibility response');
  state.value=result.responsibility;
}
onMounted(()=>load().catch(e=>error.value=String(e)));
async function edit(selected:model.ResponsibilityFacet) {
  error.value=''; busy.value=true;
  try {
    await load(true); facet.value=selected;
    const assignment=state.value?.assignments.find(a=>a.facet===selected);
    mode.value=assignment?.mode ?? model.AssignmentMode.EXPLICIT;
    party.value=model.PartyRef.create(assignment?.party ?? {kind:model.PartyKind.TEAM});
    contact.value=assignment?.contact ?? ''; source.value=assignment?.sourceRelation ?? '';
    preview.value=undefined; prepared.value=undefined; editing.value=true;
  } catch(e){error.value=String(e);} finally{busy.value=false;}
}
async function review() {
  error.value='';busy.value=true;
  try {
    const selected=sources.value.find(s=>s.relation?.relationId===source.value)?.relation;
    const change=model.ResponsibilityAssignment.create({facet:facet.value,mode:mode.value,
      party:mode.value===model.AssignmentMode.EXPLICIT ? party.value : undefined,
      contact:mode.value===model.AssignmentMode.EXPLICIT && contact.value ? contact.value : undefined,
      sourceRelation:mode.value===model.AssignmentMode.INHERIT ? selected?.relationId : undefined,
      sourceRevision:mode.value===model.AssignmentMode.INHERIT ? selected?.revision : undefined,
    });
    const request=model.Responsibility.create({target:props.item.target,expected:state.value?.item?.revision,changes:[change],action:model.ResponsibilityAction.PREVIEW});
    const result=await call({oneofKind:'responsibility',responsibility:request},working());
    if(result.oneofKind!=='responsibility') throw Error('Unexpected preview response');
    preview.value=result.responsibility; prepared.value=request;
  } catch(e){error.value=String(e);} finally{busy.value=false;}
}
async function accept() {
  if(!prepared.value)return;
  error.value='';busy.value=true;
  try {
    const result=await call({oneofKind:'responsibility',responsibility:{...prepared.value,action:model.ResponsibilityAction.APPLY}},working());
    if(result.oneofKind!=='responsibility')throw Error('Unexpected responsibility response');
    state.value=result.responsibility;editing.value=false;emit('changed');
  }catch(e){error.value=String(e);preview.value=undefined;}finally{busy.value=false;}
}
function download() {
  if(!state.value)return;
  const changes=state.value.assignments.filter(a=>a.mode!==model.AssignmentMode.INHERIT || a.sourceRelation!==undefined);
  const transfer=model.ResponsibilityTransfer.create({entries:changes.length ? [model.Responsibility.create({target:props.item.target,expected:state.value.item?.revision,changes,action:model.ResponsibilityAction.APPLY})] : []});
  const url=URL.createObjectURL(new Blob([JSON.stringify(model.ResponsibilityTransfer.toJson(transfer),null,2)],{type:'application/json'}));
  const link=document.createElement('a');link.href=url;link.download='responsibility.json';link.click();URL.revokeObjectURL(url);
}
async function reviewImport() {
  busy.value=true;error.value='';
  try {
    if(new TextEncoder().encode(json.value).length>256*1024)throw Error('Import exceeds 256 KiB');
    const transfer=model.ResponsibilityTransfer.fromJson(JSON.parse(json.value));
    await load();
    const result=await call({oneofKind:'responsibilityTransfer',responsibilityTransfer:{entries:transfer.entries.map(e=>({...e,action:model.ResponsibilityAction.PREVIEW}))}},working());
    if(result.oneofKind!=='responsibilityTransfer')throw Error('Unexpected import preview');
    imported.value=transfer;importPreview.value=result.responsibilityTransfer;
  }catch(e){error.value=String(e);}finally{busy.value=false;}
}
async function acceptImport() {
  if(!imported.value)return;
  busy.value=true;error.value='';
  try {
    await call({oneofKind:'responsibilityTransfer',responsibilityTransfer:{entries:imported.value.entries.map(e=>({...e,action:model.ResponsibilityAction.APPLY}))}},working());
    importing.value=false;emit('changed');await load();
  }catch(e){error.value=String(e);importPreview.value=undefined;}finally{busy.value=false;}
}
</script>
<template>
<section aria-label="Ownership and responsibility"><h3>Ownership &amp; responsibility</h3>
<p class="hint">Assignments are claims and responsibilities. They do not grant access or prove legal ownership.</p>
<p v-if="error && !editing && !importing" role="alert">{{error}}</p>
<dl v-if="state"><template v-for="entry in state.effective" :key="entry.facet"><dt>{{names[entry.facet]}}</dt><dd>
  <strong v-if="entry.party">{{entry.party.name}}</strong><span v-else>{{status(entry.status)}}</span>
  <small v-if="entry.party">{{status(entry.status)}} · {{entry.party.issuer}} / {{entry.party.subject}}</small>
  <span v-if="entry.source"><button @click="emit('open',entry.source)">Assignment source</button></span>
  <span v-if="entry.contact">Contact: {{entry.contact}}</span>
</dd></template></dl>
<p v-if="state?.writable.some(f=>!state?.fullyReadable.includes(f))" class="hint">Editing a complete assignment requires permission to read its declaration and contact.</p>
<div v-if="state" class="actions"><button v-for="f in facets.filter(f=>state?.writable.includes(f)&&state?.fullyReadable.includes(f))" :key="f" :disabled="busy" @click="edit(f)">Edit {{names[f].toLowerCase()}}</button><button @click="download">Export responsibility</button><button v-if="state.writable.length" @click="importing=true;error='';importPreview=undefined">Import responsibility</button></div>
</section>
<Dialog v-if="editing" :title="'Edit '+names[facet].toLowerCase()" @close="editing=false"><p v-if="error" role="alert">{{error}}</p>
<form v-if="!preview" @submit.prevent="review"><label for="responsibility-assignment">Assignment</label><select id="responsibility-assignment" v-model.number="mode"><option :value="model.AssignmentMode.EXPLICIT">Explicit assignment</option><option v-if="inheritable" :value="model.AssignmentMode.INHERIT">Inherit from a qualified source</option><option :value="model.AssignmentMode.UNASSIGNED">Unassigned, stop inheritance</option></select>
<template v-if="mode===model.AssignmentMode.EXPLICIT"><label for="responsibility-party-kind">Party kind</label><select id="responsibility-party-kind" v-model.number="party.kind"><option :value="model.PartyKind.INDIVIDUAL">Individual</option><option :value="model.PartyKind.ORGANIZATION">Organization</option><option :value="model.PartyKind.TEAM">Team</option></select><label>Party namespace<input v-model="party.issuer" required maxlength="256"></label><label>Party identity<input v-model="party.subject" required maxlength="256"></label><label>Party label<input v-model="party.name" required maxlength="160"></label><label>Contact route<input v-model="contact" maxlength="512"></label></template>
<template v-if="mode===model.AssignmentMode.INHERIT"><label for="responsibility-source">Inheritance source</label><select id="responsibility-source" v-model="source"><option value="">Use the unique qualified component source, if present</option><option v-for="s in sources" :key="s.relation?.relationId" :value="s.relation?.relationId">{{display(s.parent?.fields.find(f=>f.name==='name'))}} · {{s.parent?.target?.assetId}}</option></select></template>
<p v-if="mode===model.AssignmentMode.INHERIT" class="hint">Selecting a hosting relationship explicitly establishes administrative responsibility. Future VM placement changes will not transfer it.</p>
<div class="actions"><button type="button" @click="editing=false">Cancel</button><button class="primary" :disabled="busy">Review assignment</button></div></form>
<template v-else><h3>Review effective result</h3><p>{{names[facet]}}: {{preview.effective.find(e=>e.facet===facet)?.party?.name ?? status(preview.effective.find(e=>e.facet===facet)?.status ?? model.ResponsibilityStatus.UNKNOWN)}}</p><p class="hint">Current basis {{state?.revision.toString()}}. Access grants and credentials are unchanged.</p><div class="actions"><button @click="preview=undefined">Back to edit</button><button class="primary" :disabled="busy" @click="accept">Apply assignment</button></div></template>
</Dialog>
<Dialog v-if="importing" title="Import responsibility" @close="importing=false"><p v-if="error" role="alert">{{error}}</p><p class="hint">Import explicit declarations for existing resources in this scope. Revisions and field rights are checked again at acceptance. Exports omit hidden source bindings.</p><form v-if="!importPreview" @submit.prevent="reviewImport"><label>Typed JSON import<textarea v-model="json" required rows="12" maxlength="262144" /></label><button class="primary" :disabled="busy">Review import</button></form><template v-else><p>{{importPreview.entries.length}} resources validated. No changes have been saved.</p><ul><li v-for="entry in importPreview.entries" :key="key(entry.item?.target)">{{entry.item?.target?.assetId}}<span v-for="value in entry.effective.filter(e=>e.party)" :key="value.facet"> · {{names[value.facet]}}: {{value.party?.name}}</span></li></ul><button @click="importPreview=undefined">Back to edit</button><button class="primary" :disabled="busy" @click="acceptImport">Apply import</button></template></Dialog>
</template>
<style scoped>small,dd>span{display:block;overflow-wrap:anywhere}dd{margin-bottom:12px}textarea{box-sizing:border-box;width:100%;font:inherit}label{display:grid;gap:5px;margin:12px 0}input,select{width:100%;box-sizing:border-box;min-width:0}form{min-width:0}</style>
