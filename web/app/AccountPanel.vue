<script setup lang="ts">
import { computed, onMounted, ref } from 'vue';
import Dialog from './Dialog.vue';
import { call,model,AssetKind,OperationState,EffectOutcome,display,key,text,type AssetRef } from './api';
const props=defineProps<{item:model.Item;working:model.WorkingContext}>();
const emit=defineEmits<{changed:[];open:[target:AssetRef];share:[]}>();
const state=ref<model.ExternalAccountState>(); const error=ref(''); const busy=ref(false);
const editor=ref<''|'metadata'|'binding'|'binding-review'|'request'>('');
const bindingReview=ref<{link:model.AccountLink;removing:boolean}>();
const metadata=ref(model.AccountMetadata.create());
const search=ref('');const matches=ref<model.Item[]>([]);const chosen=ref<model.Item>();const role=ref(model.AccountLinkKind.MANAGED_RESOURCE);
const target=ref('');const change=ref('');const field=ref('description');const value=ref('');const deadline=ref('');
const selectedRequest=ref<model.OwnerRequestState>(); const outcome=ref(EffectOutcome.UNSPECIFIED);
const outcomes=[EffectOutcome.UNKNOWN,EffectOutcome.CONFIRMED,EffectOutcome.CONFIRMED_NO_EFFECT];
const draftRequest=ref<model.OwnerRequest>();
const can=(cap:model.AccountCapability)=>state.value?.authorized.includes(cap) ?? false;
const metadataFields=['issuer','subject','account_kind','delegation','login','sender','credential_binding_id','recovery_route'];
// This form replaces metadata, so every omitted value must be known absent rather than hidden.
const canMetadata=computed(()=>state.value?.metadataFullyReadable&&metadataFields.every(f=>state.value?.writable.includes(f)));
const managed=computed(()=>state.value?.links.filter(l=>l.kind===model.AccountLinkKind.MANAGED_RESOURCE) ?? []);
const capabilities=[
  {id:model.AccountCapability.SHARE_REFERENCE,label:'Share inventory reference'},
  {id:model.AccountCapability.WORKLOAD_USE,label:'Use credential in a workload'},
  {id:model.AccountCapability.REVEAL_SECRET,label:'Reveal credential to a person'},
  {id:model.AccountCapability.ROTATE,label:'Rotate or reset'},
  {id:model.AccountCapability.DELEGATE,label:'Delegate access'},
  {id:model.AccountCapability.REQUEST_CHANGE,label:'Request a change'},
  {id:model.AccountCapability.APPROVE_CHANGE,label:'Approve a change'},
  {id:model.AccountCapability.EXECUTE_CHANGE,label:'Perform an owner action'},
];
function working(){return model.WorkingContext.create({...props.working,expected:state.value?.revision});}
function related(target:AssetRef|undefined){return state.value?.related.find(i=>key(i.target)===key(target)) ?? matches.value.find(i=>key(i.target)===key(target));}
function name(target:AssetRef|undefined){const item=related(target);return display(item?.fields.find(f=>f.name==='name')) || target?.assetId || '';}
async function load(cursor?:string){
  const result=await call({oneofKind:'externalAccount',externalAccount:model.ExternalAccount.create({target:props.item.target,requestsCursor:cursor})},working());
  if(result.oneofKind!=='externalAccount')throw Error('Unexpected account response');state.value=result.externalAccount;
}
onMounted(()=>load().catch(e=>error.value=String(e)));
function editMetadata(){metadata.value=state.value?.metadata ? model.AccountMetadata.create(state.value.metadata) : model.AccountMetadata.create();editor.value='metadata';error.value='';}
async function mutate(request:Partial<model.ExternalAccount>,context:model.WorkingContext=working()){
  error.value='';busy.value=true;
  try{const result=await call({oneofKind:'externalAccount',externalAccount:model.ExternalAccount.create({...request,target:props.item.target,expected:state.value?.item?.revision})},context);if(result.oneofKind!=='externalAccount')throw Error('Unexpected account response');state.value=result.externalAccount;editor.value='';emit('changed');}
  catch(e){error.value=String(e);}finally{busy.value=false;}
}
function reviewBinding(link:model.AccountLink,removing=false){bindingReview.value={link,removing};error.value='';editor.value='binding-review';}
async function applyBinding(){
  const review=bindingReview.value;
  if(!review?.link.target||!props.item.target)return;
  const context=model.WorkingContext.create({roots:[props.item.target,review.link.target],expected:state.value?.revision});
  await mutate(review.removing?{remove:[review.link]}:{add:[review.link]},context);
}
async function find(){
  error.value='';busy.value=true;
  try{const result=await call({oneofKind:'query',query:model.Query.create({search:search.value.trim(),limit:10,columns:['name']})});if(result.oneofKind!=='page')throw Error('Unexpected search response');matches.value=result.page.items.filter(i=>key(i.target)!==key(props.item.target));chosen.value=undefined;}
  catch(e){error.value=String(e);}finally{busy.value=false;}
}
function newRequest(){editor.value='request';draftRequest.value=undefined;error.value='';target.value='';change.value='';value.value='';deadline.value='';}
async function requestChange(){
  error.value='';busy.value=true;
  try{
    if(!draftRequest.value){
      const selected=state.value?.related.find(i=>key(i.target)===target.value);
      if(!selected?.target)throw Error('Select the exact managed resource');
      const expires=Date.parse(deadline.value);
      if(!Number.isFinite(expires)||expires<=Date.now())throw Error('Select a future request deadline');
      draftRequest.value=model.OwnerRequest.create({action:model.OwnerRequestAction.CREATE,account:props.item.target,accountRevision:state.value?.item?.revision,target:selected.target,targetRevision:selected.revision,change:change.value,arguments:[text(field.value,value.value)],retryKey:crypto.randomUUID(),expiresAtMs:BigInt(expires)});
    }
    const result=await call({oneofKind:'ownerRequest',ownerRequest:draftRequest.value},working());
    if(result.oneofKind!=='ownerRequest')throw Error('Unexpected owner request response');
    selectedRequest.value=result.ownerRequest;editor.value='';await load();
  }catch(e){error.value=String(e);}finally{busy.value=false;}
}
async function openRequest(operationId:string){
  error.value='';busy.value=true;
  try{const result=await call({oneofKind:'ownerRequest',ownerRequest:model.OwnerRequest.create({action:model.OwnerRequestAction.READ,account:props.item.target,operationId})},working());if(result.oneofKind!=='ownerRequest')throw Error('Unexpected owner request response');selectedRequest.value=result.ownerRequest;outcome.value=EffectOutcome.UNSPECIFIED;}
  catch(e){error.value=String(e);}finally{busy.value=false;}
}
async function transition(action:model.OwnerRequestAction){
  error.value='';busy.value=true;
  try{
    const result=await call({oneofKind:'ownerRequest',ownerRequest:model.OwnerRequest.create({action,account:props.item.target,accountRevision:state.value?.item?.revision,operationId:selectedRequest.value?.operation?.operationId,expected:selectedRequest.value?.operation?.revision,reportedOutcome:action===model.OwnerRequestAction.REPORT?outcome.value:EffectOutcome.UNSPECIFIED})},working());
    if(result.oneofKind!=='ownerRequest')throw Error('Unexpected owner request response');selectedRequest.value=result.ownerRequest;await load();
  }catch(e){error.value=String(e);}finally{busy.value=false;}
}
function linkName(kind:model.AccountLinkKind){return model.AccountLinkKind[kind]?.toLowerCase().replaceAll('_',' ');}
</script>
<template><section aria-label="External account"><h3>External account</h3><p v-if="error&&!editor&&!selectedRequest" role="alert">{{error}}</p>
<template v-if="state"><dl v-if="state.metadata"><dt v-if="state.metadata.issuer">Provider namespace</dt><dd v-if="state.metadata.issuer">{{state.metadata.issuer}}</dd><dt v-if="state.metadata.subject">Provider identity</dt><dd v-if="state.metadata.subject">{{state.metadata.subject}}</dd><dt>Account kind</dt><dd>{{model.ExternalAccountKind[state.metadata.kind]?.toLowerCase().replaceAll('_',' ')}}</dd><template v-if="state.metadata.login"><dt>Login label</dt><dd>{{state.metadata.login}}</dd></template><template v-if="state.metadata.sender"><dt>Sender constraint</dt><dd>{{state.metadata.sender}}</dd></template></dl>
<p v-if="state.metadata?.delegation===model.AccountDelegation.OWNER_ONLY" class="callout">Account owner action required. Company approval cannot give access to this personal provider account.</p>
<p v-else-if="state.metadata?.delegation===model.AccountDelegation.SCOPED" class="hint">Scoped delegation is recorded. Actual use still requires a qualified provider connection and current authority.</p>
<p v-else class="hint">Provider delegation capability is unknown.</p>
<p v-if="state.metadata?.recoveryRoute">Provider recovery / transfer route: {{state.metadata.recoveryRoute}}</p>
<p v-if="!canMetadata&&metadataFields.some(f=>state?.writable.includes(f))" class="hint">Replacing metadata requires permission to read and edit every metadata field. Bindings are edited separately.</p>
<div class="actions"><button v-if="canMetadata" @click="editMetadata">Edit account metadata</button><button v-if="state.writable.includes('account.bindings')" @click="editor='binding';matches=[];chosen=undefined;search='';error=''">Bind selected resource</button><button v-if="can(model.AccountCapability.SHARE_REFERENCE)" @click="emit('share')">Share account view</button><button v-if="can(model.AccountCapability.REQUEST_CHANGE)&&state.metadata?.delegation===model.AccountDelegation.OWNER_ONLY&&managed.length" @click="newRequest">Request owner action</button></div>
<h4>Selected bindings</h4><p v-if="!state.links.length" class="hint">No bindings are visible in this context. This does not prove the account has no other resources.</p><ul><li v-for="link in state.links" :key="link.kind+'/'+key(link.target)">{{linkName(link.kind)}}: <button v-if="link.target" @click="emit('open',link.target)">{{name(link.target)}}</button><button v-if="state.writable.includes('account.bindings')" :disabled="busy" @click="reviewBinding(link,true)">Remove binding</button></li></ul>
<details><summary>Independent capabilities</summary><p class="hint">These are separate host decisions. This inventory card neither fetches credentials nor enables provider or vault effects.</p><dl><template v-for="cap in capabilities" :key="cap.id"><dt>{{cap.label}}</dt><dd>{{can(cap.id)?'Authorized in this context':'Not authorized in this context'}}</dd></template></dl></details>
<h4>Owner requests</h4><p class="hint">An unavailable owner leaves the request unresolved. Owner reports require independent provider verification.</p><ul><li v-for="request in state.requests" :key="request.operationId"><button @click="openRequest(request.operationId)">{{name(request.target)}} · {{request.state===OperationState.BLOCKED?'Waiting for owner action':request.state===OperationState.NEEDS_ATTENTION?'Needs independent verification':'Current request'}}</button></li></ul><button v-if="state.requestsNext" @click="load(state.requestsNext).catch(e=>error=String(e))">Next requests</button>
</template></section>
<Dialog v-if="editor==='metadata'" title="Account metadata" @close="editor=''">
  <p v-if="error" role="alert">{{error}}</p><p class="hint">Record only this selected account. Do not enter passwords, tokens or recovery answers.</p>
  <form @submit.prevent="mutate({metadata})">
    <label>Provider namespace<input v-model="metadata.issuer" required maxlength="256" :readonly="!!state?.metadata?.issuer"></label>
    <label>Provider account identity<input v-model="metadata.subject" required maxlength="256" :readonly="!!state?.metadata?.subject"></label>
    <label for="external-account-kind">Account kind</label>
    <select id="external-account-kind" v-model.number="metadata.kind" required><option :value="model.ExternalAccountKind.UNSPECIFIED" disabled>Select account kind</option><option :value="model.ExternalAccountKind.ADMINISTRATION">Human administration account</option><option :value="model.ExternalAccountKind.PROGRAMMATIC">Programmatic service account</option></select>
    <label for="external-account-delegation">Provider delegation</label>
    <select id="external-account-delegation" v-model.number="metadata.delegation"><option :value="model.AccountDelegation.UNSPECIFIED">Unknown</option><option :value="model.AccountDelegation.OWNER_ONLY">Owner action only</option><option :value="model.AccountDelegation.SCOPED">Scoped delegation supported</option></select>
    <label>Login label<input :value="metadata.login" maxlength="256" @input="metadata.login=($event.target as HTMLInputElement).value||undefined"></label>
    <label>Sender constraint<input :value="metadata.sender" maxlength="256" @input="metadata.sender=($event.target as HTMLInputElement).value||undefined"></label>
    <label>Credential binding UUID<input :value="metadata.credentialBindingId" maxlength="36" @input="metadata.credentialBindingId=($event.target as HTMLInputElement).value||undefined"></label>
    <label>Provider recovery route<input :value="metadata.recoveryRoute" maxlength="512" @input="metadata.recoveryRoute=($event.target as HTMLInputElement).value||undefined"></label>
    <button class="primary" :disabled="busy">Save account metadata</button>
  </form>
</Dialog>
<Dialog v-if="editor==='binding'" title="Bind selected resource" @close="editor=''">
  <p v-if="error" role="alert">{{error}}</p><p class="hint">Select an existing permitted resource, then review both endpoints in a broader relationship context. This reference changes no ownership or grants.</p>
  <form @submit.prevent="find"><label>Find resource<input v-model="search" required minlength="2" maxlength="256"></label><button :disabled="busy">Find resources</button></form>
  <ul><li v-for="item in matches" :key="key(item.target)"><button :aria-pressed="key(chosen?.target)===key(item.target)" @click="chosen=item">{{display(item.fields.find(f=>f.name==='name'))}} · {{AssetKind[item.kind]?.toLowerCase()}}</button></li></ul>
  <label for="account-binding-role">Binding role</label><select id="account-binding-role" v-model.number="role"><option :value="model.AccountLinkKind.MANAGED_RESOURCE">Managed resource</option><option :value="model.AccountLinkKind.SERVICE">Owning service</option><option :value="model.AccountLinkKind.ENDPOINT">Service endpoint</option><option :value="model.AccountLinkKind.CONSUMER">Consumer workload</option></select>
  <button class="primary" :disabled="busy||!chosen" @click="reviewBinding(model.AccountLink.create({kind:role,target:chosen?.target}))">Review binding</button>
</Dialog>
<Dialog v-if="editor==='binding-review'&&bindingReview" title="Review account binding" @close="editor=''">
  <p v-if="error" role="alert">{{error}}</p>
  <section aria-label="Selected relationship context"><h3>Selected relationship context</h3>
    <p>{{display(item.fields.find(f=>f.name==='name'))}} → {{name(bindingReview.link.target)}}</p>
    <p>{{linkName(bindingReview.link.kind)}} · {{bindingReview.removing?'Remove binding':'Add binding'}}</p>
  </section>
  <p class="hint">Only this selected pair is changed. Each endpoint still requires current permission. Ownership and access grants are unchanged.</p>
  <div class="actions"><button @click="editor=bindingReview.removing?'':'binding'">Back</button><button class="primary" :disabled="busy" @click="applyBinding">{{bindingReview.removing?'Confirm removal':'Save binding'}}</button></div>
</Dialog>
<Dialog v-if="editor==='request'" title="Request account owner action" @close="editor=''">
  <p v-if="error" role="alert">{{error}}</p>
  <form @submit.prevent="requestChange">
    <fieldset :disabled="!!draftRequest"><legend>Exact proposed change</legend>
      <label for="owner-request-target">Managed resource</label><select id="owner-request-target" v-model="target" required><option value="" disabled>Select resource</option><option v-for="link in managed" :key="key(link.target)" :value="key(link.target)">{{name(link.target)}}</option></select>
      <label>Requested action<input v-model="change" required maxlength="64" pattern="[A-Za-z0-9_.-]+"></label>
      <label>Proposed field<input v-model="field" required maxlength="64"></label>
      <label>Proposed value<input v-model="value" required maxlength="4096"></label>
      <label>Request deadline<input v-model="deadline" required type="datetime-local"></label>
    </fieldset>
    <p class="hint">The exact target, change and revisions are retained as a durable request. No external action is dispatched.</p>
    <button class="primary" :disabled="busy">{{draftRequest?'Retry same request':'Save owner request'}}</button>
  </form>
</Dialog>
<Dialog v-if="selectedRequest" title="Owner action request" @close="selectedRequest=undefined">
  <p v-if="error" role="alert">{{error}}</p><p>{{selectedRequest.change}} · {{name(selectedRequest.operation?.target)}}</p>
  <dl><template v-for="argument in selectedRequest.arguments" :key="argument.name"><dt>{{argument.name}}</dt><dd>{{display(argument)}}</dd></template></dl>
  <p>Target basis: {{selectedRequest.operation?.inventoryRevision.toString()}}</p>
  <p v-if="!selectedRequest.argumentsComplete" class="hint">Some proposed fields are not disclosable. This projection cannot approve the full change.</p>
  <p v-if="selectedRequest.ownerReported" class="callout">Owner-reported result. Independent provider verification is still required.</p>
  <p v-else>{{selectedRequest.approved?'Approved; waiting for the account owner.':'Waiting for independent approval.'}}</p>
  <button v-if="!selectedRequest.approved&&selectedRequest.argumentsComplete&&can(model.AccountCapability.APPROVE_CHANGE)" :disabled="busy" @click="transition(model.OwnerRequestAction.APPROVE)">Approve requested change</button>
  <form v-if="selectedRequest.approved&&selectedRequest.argumentsComplete&&can(model.AccountCapability.EXECUTE_CHANGE)" @submit.prevent="transition(model.OwnerRequestAction.REPORT)">
    <label for="owner-reported-outcome">Owner-reported outcome</label><select id="owner-reported-outcome" v-model.number="outcome" required><option :value="EffectOutcome.UNSPECIFIED" disabled>Select outcome explicitly</option><option v-for="option in outcomes" :key="option" :value="option">{{EffectOutcome[option]?.toLowerCase().replaceAll('_',' ')}}</option></select>
    <button :disabled="busy||outcome===EffectOutcome.UNSPECIFIED">Record owner report</button>
  </form>
</Dialog>
</template>
<style scoped>label{display:grid;gap:5px;margin:12px 0}input,select{box-sizing:border-box;width:100%;min-width:0}li{margin:8px 0;overflow-wrap:anywhere}fieldset{border:0;padding:0;min-width:0}.callout{padding:12px;background:#f8f0d5;border-radius:8px}dd{overflow-wrap:anywhere}</style>
