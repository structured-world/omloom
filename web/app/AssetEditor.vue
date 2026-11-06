<script setup lang="ts">
import { computed, onMounted, onBeforeUnmount, nextTick, reactive, ref, watch } from 'vue';
import { Field as WireField } from '../src/generated/omloom/v1/contracts';
import Dialog from './Dialog.vue';
import PlacementPicker from './PlacementSearch.vue';
import InitialAddresses from './InitialAddresses.vue';
import AddressPrefix from './AddressPrefix.vue';
import ObserveHypervisor from './ObserveHypervisor.vue';
import EquipmentField from './EquipmentField.vue';
import { equipmentFields as equipmentNames } from './equipment-fields';
import { softwareFields, softwareChoiceLabel } from './software-fields';
import { call, typedField, input, display, label, key, model, AssetKind, type AssetRef } from './api';

const props = defineProps<{ item?: model.Item; definitions: model.Definition[]; initialKind?: AssetKind; initialHypervisor?: boolean; working: model.WorkingContext; view: model.View; fieldMask?: readonly string[]; question?: string; receipt?: {name:string;submit:(changes:import('./api').Field[],working:model.WorkingContext,next:boolean)=>Promise<void>}; inline?:boolean; inactive?:boolean }>();
const emit = defineEmits<{ close: []; saved: [target?: AssetRef]; recheck: [] }>();
const focused = computed(() => props.fieldMask !== undefined);
const form = ref<HTMLFormElement>();
let entryFocus:Element|null=null;
let focusPending=true;
const reviewedItem = ref(props.item);
const outcome = ref<'' | 'conflict' | 'uncertain' | 'reviewed' | 'denied'>('');
const uncertain = ref(false);
let mounted = true;
onBeforeUnmount(() => { mounted = false; });
const editWorking = model.WorkingContext.create(props.working);
const included = (name: string) => props.fieldMask === undefined || props.fieldMask.includes(name);
const kind = ref<number | undefined>(props.item?.kind ?? props.initialKind);
const actions = ref<model.Actions>();
const selectedParent = ref('');
const hypervisor = ref(!!props.initialHypervisor || (!props.item && props.view === model.View.VIRTUALIZATION && props.initialKind === AssetKind.SOFTWARE_INSTALLATION));
const placement = ref<model.Item>(); const placementReady = ref(true); const placementBusy = ref(false);
const placementActions = ref<model.Actions>();
const initialAddresses = ref([model.InitialAddress.create()]);
const prefixChoice = ref<Partial<model.Address>>({});
const prefixEditor = ref<InstanceType<typeof AddressPrefix>>();
const addressReview = ref<model.AddressReview>();
const reviewedAddress = ref<model.Address>();
const reviewedMeaning = computed(() => {
  const meaning = reviewedAddress.value?.meaning;
  return meaning === undefined ? undefined : model.AddressMeaning[meaning]?.toLowerCase().replaceAll('_', ' ');
});
const reviewedWorking = ref<model.WorkingContext>();
const derivedBroadcast = computed(() => {
  const review = addressReview.value;
  if (!review?.address || review.prefixLength === undefined || review.prefixLength >= 31 || review.address.includes(':')) return undefined;
  const octets = review.address.split('.').map(Number);
  const value = octets.reduce((ip, octet) => ip * 256 + octet, 0);
  const hosts = 2 ** (32 - review.prefixLength);
  const broadcast = Math.floor(value / hosts) * hosts + hosts - 1;
  return [24,16,8,0].map(shift => Math.floor(broadcast / 2 ** shift) % 256).join('.');
});
const values = reactive<Record<string, string>>({});
const equipmentChanges = reactive<Record<string, import('./api').Field | undefined>>({});
const equipmentValidity = reactive<Record<string,boolean>>({});
const selectedEquipment = ref(props.fieldMask?.filter(f=>equipmentNames.includes(f)) ?? props.item?.fields.filter(f=>equipmentNames.includes(f.name)).map(f=>f.name) ?? []);
const equipmentToAdd = ref('');
const equipmentFields = computed(()=>(kind.value===AssetKind.DEVICE||kind.value===AssetKind.PHYSICAL_COMPONENT) ? (props.item ? actions.value?.fields : choice.value?.fields)?.filter(f=>equipmentNames.includes(f)&&included(f)) ?? [] : []);
function addEquipment(){if(equipmentFields.value.includes(equipmentToAdd.value)&&!selectedEquipment.value.includes(equipmentToAdd.value))selectedEquipment.value.push(equipmentToAdd.value);equipmentToAdd.value='';}
const modes = reactive<Record<string, string>>({});
const submittedMask = computed(() => modes.prefix_length !== 'clear' && values.prefix_length !== undefined && values.prefix_length !== '' ? Number(values.prefix_length) : undefined);
const error = ref(''); const busy = ref(false); const preparing = ref(true); const more = ref(false);
watch([preparing,()=>props.inactive],async()=>{
  if(preparing.value||props.inactive||!focusPending||(!focused.value&&!props.receipt))return;
  await nextTick();
  if(!mounted||preparing.value||props.inactive)return;
  focusPending=false;
  // Never steal input begun while preparation or the enclosing refresh was pending.
  if(document.activeElement===entryFocus||document.activeElement===document.body)(form.value?.querySelector<HTMLElement>('[data-fact-value]')??form.value?.querySelector<HTMLElement>('#value-name,textarea,input'))?.focus();
});
async function reprepareCreation(){
  preparing.value=true;error.value='';
  try{
    const response=await call({oneofKind:'prepare',prepare:model.Prepare.create({view:props.view})},model.WorkingContext.create({...props.working,expected:undefined}));
    if(!mounted)return false;
    if(response.oneofKind!=='actions')throw Error('Inventory creation could not be prepared.');
    if(props.receipt&&!response.actions.creations.some(choice=>choice.kind===AssetKind.DEVICE&&!choice.parent))throw Error('Device creation is not permitted in this inventory context.');
    actions.value=response.actions;
    return true;
  }catch(e){error.value=String(e);return false;}finally{preparing.value=false;}
}
defineExpose({reprepareCreation});
watch(()=>props.receipt,async(value,previous)=>{
  if(!value&&previous)await reprepareCreation();
});
type EditorField = { id: string; name: string; type: model.FieldType; choices: string[]; advanced: boolean };
const parents = computed(() => [...new Map(actions.value?.creations.flatMap(c => c.parent?.target ? [[key(c.parent.target), c.parent] as const] : [])).values()]);
const choices = computed(() => actions.value?.creations.filter(c => key(c.parent?.target) === selectedParent.value) ?? []);
const baseChoice = computed(() => choices.value.find(c => c.kind === kind.value));
const choice = computed(() => placement.value ? placementActions.value?.creations.find(c => c.kind === kind.value && key(c.parent?.target) === key(placement.value?.target)) : baseChoice.value);
const initialPrefix = computed(() => choice.value?.prefixContext);
const placementKind = computed(() => !props.item && (kind.value === AssetKind.VIRTUAL_MACHINE && baseChoice.value?.parent?.kind !== AssetKind.SOFTWARE_INSTALLATION ? 'hypervisor' : !baseChoice.value?.parent && kind.value === AssetKind.SOFTWARE_INSTALLATION && (hypervisor.value || (modes.virtualization_role === 'set' && values.virtualization_role === virtualizationChoices[0]!.value)) ? 'server' : undefined));
const placementRoots = computed(() => baseChoice.value?.parent?.target ? [baseChoice.value.parent.target] : props.working.roots);
const resourceType = computed({
  get: () => kind.value === AssetKind.SOFTWARE_INSTALLATION && hypervisor.value ? 'hypervisor' : String(kind.value),
  set: (value: string) => {
    const previous = hypervisor.value;
    hypervisor.value = value === 'hypervisor' || (props.view === model.View.VIRTUALIZATION && Number(value) === AssetKind.SOFTWARE_INSTALLATION);
    kind.value = value === 'hypervisor' ? AssetKind.SOFTWARE_INSTALLATION : Number(value);
    if (hypervisor.value) { values.virtualization_role = virtualizationChoices[0]!.value; modes.virtualization_role = 'set'; }
    else if (previous) { values.virtualization_role = ''; modes.virtualization_role = 'preserve'; }
  },
});
const known = computed(() => kind.value === AssetKind.ADDRESS_REFERENCE);
const virtualizationChoices = [
  { value: `VIRTUALIZATION_ROLE_${model.VirtualizationRole[model.VirtualizationRole.HYPERVISOR]}`, label: 'Installed and enabled' },
  { value: `VIRTUALIZATION_ROLE_${model.VirtualizationRole[model.VirtualizationRole.NOT_HYPERVISOR]}`, label: 'Not installed or disabled' },
];
const roleInput = computed({
  get: () => modes.virtualization_role === 'preserve' ? props.item ? 'preserve' : 'unknown' : modes.virtualization_role === 'clear' ? 'unknown' : values.virtualization_role,
  set: (value: string) => {
    if (value === 'preserve') modes.virtualization_role = 'preserve';
    else if (value === 'unknown') { modes.virtualization_role = props.item ? 'clear' : 'preserve'; values.virtualization_role = ''; }
    else { values.virtualization_role = value; modes.virtualization_role = 'set'; }
  },
});
const fields = computed<EditorField[]>(() => {
  const allowed = props.item ? actions.value?.fields : (choice.value?.fields ?? baseChoice.value?.fields);
  if (!allowed) return [];
  const builtins: EditorField[] = known.value ? [
    { id: 'address', name: 'IP address', type: model.FieldType.TEXT, choices: [], advanced: false },
    { id: 'prefix_length', name: 'Known prefix length', type: model.FieldType.INTEGER, choices: [], advanced: true },
    { id: 'vlan_tag', name: 'Known local VLAN tag', type: model.FieldType.INTEGER, choices: [], advanced: true },
    { id: 'meaning', name: 'Address meaning', type: model.FieldType.CHOICE, choices: Object.values(model.AddressMeaning).filter((v): v is number => typeof v === 'number').map(v => `ADDRESS_MEANING_${model.AddressMeaning[v]!}`), advanced: true },
    { id: 'zone', name: 'Known local IPv6 zone', type: model.FieldType.TEXT, choices: [], advanced: true },
    { id: 'broadcast', name: 'Explicit endpoint broadcast', type: model.FieldType.TEXT, choices: [], advanced: true },
    { id: 'description', name: 'Description', type: model.FieldType.TEXT, choices: [], advanced: true },
  ] : [
    { id: 'name', name: 'Name', type: model.FieldType.TEXT, choices: [], advanced: false },
    { id: 'dns_name', name: 'DNS name', type: model.FieldType.TEXT, choices: [], advanced: false },
    { id: 'description', name: 'Description', type: model.FieldType.TEXT, choices: [], advanced: false },
    { id: 'serial', name: 'Serial number', type: model.FieldType.TEXT, choices: [], advanced: false },
    { id: 'virtualization_role', name: 'Virtualization role', type: model.FieldType.CHOICE, choices: virtualizationChoices.map(v => v.value), advanced: false },
  ];
  const custom = props.definitions.filter(d => !d.retired && kind.value !== undefined && d.kinds.includes(kind.value))
    .map(d => ({ id: d.id, name: d.name, type: d.type, choices: d.choices, advanced: false }));
  return [...builtins, ...softwareFields(allowed), ...custom].filter(f => allowed.includes(f.id) && included(f.id) && !equipmentFields.value.includes(f.id));
});
watch(fields, available => {
  for (const f of available) {
    const first = !(f.id in values);
    if (!(f.id in values)) values[f.id] = input(props.item?.fields.find(v => v.name === f.id));
    if (first && f.id === 'name' && props.receipt) values[f.id] = props.receipt.name;
    if (!(f.id in modes)) modes[f.id] = props.item ? 'preserve' : ['name', 'address'].includes(f.id) ? 'set' : 'preserve';
    if (!props.item && hypervisor.value && f.id === 'virtualization_role' && first) { values[f.id] = virtualizationChoices[0]!.value; modes[f.id] = 'set'; }
  }
}, { immediate: true });
watch([kind, hypervisor, selectedParent, placementKind], () => { placement.value = undefined; placementActions.value = undefined; placementReady.value = true; placementGeneration++; placementBusy.value = false; });
let placementGeneration = 0;
async function selectPlacement(item?: model.Item) {
  const turn = ++placementGeneration;
  placement.value = item; placementActions.value = undefined; error.value = ''; placementBusy.value = !!item;
  if (!item?.target) return;
  try {
    const reply = await call({ oneofKind: 'prepare', prepare: model.Prepare.create({ view: props.view }) }, model.WorkingContext.create({ ...props.working, focus: item.target, expected: undefined }));
    if (reply.oneofKind !== 'actions') throw Error('Placement cannot be prepared');
    if (!reply.actions.creations.some(c => c.kind === kind.value && key(c.parent?.target) === key(item.target))) throw Error('You cannot create this resource at the selected placement. Choose another placement.');
    if (turn === placementGeneration) placementActions.value = reply.actions;
  } catch (e) { if (turn === placementGeneration) error.value = String(e); }
  finally { if (turn === placementGeneration) placementBusy.value = false; }
}
watch(choices, available => {
  if (!props.item && !available.some(c => c.kind === kind.value)) kind.value = available[0]?.kind;
});
const parentLabel = (parent: model.Item) => input(parent.fields.find(f => f.name === 'name')) || parent.target!.assetId.slice(-8);
function useProxmox() {
  if (!values.name) values.name = 'Proxmox VE';
  modes.name = 'set';
  values.virtualization_role = virtualizationChoices[0]!.value;
  modes.virtualization_role = 'set';
}
function useObservation(value: { name?: string; addresses: model.InitialAddress[] }) {
  const current = initialAddresses.value.filter(a => a.address.trim() || a.vlanTag !== undefined);
  const merged = [...current];
  for (const address of value.addresses) {
    if (!merged.some(a => a.address === address.address && a.interfaceName === address.interfaceName && a.vlanTag === address.vlanTag)) merged.push(model.InitialAddress.create(address));
  }
  if (merged.length > 16) { error.value = 'Keep at most 16 addresses in this creation. Existing input was preserved.'; return; }
  initialAddresses.value = merged;
  if (value.name !== undefined) { values.name = value.name; modes.name = 'set'; }
}
onMounted(async () => {
  entryFocus=document.activeElement;
  try {
    if (focused.value && (props.fieldMask?.length !== 1 || props.item?.kind !== AssetKind.DEVICE)) throw Error('Choose one accepted device fact.');
    const working = model.WorkingContext.create({ ...props.working, focus: props.item?.target ?? props.working.focus, expected: undefined });
    const reply = await call({ oneofKind: 'prepare', prepare: model.Prepare.create({ view: props.view }) }, working);
    if (reply.oneofKind !== 'actions') throw Error('Creation context is unavailable');
    if (!mounted) return;
    actions.value = reply.actions;
    if (focused.value && !props.fieldMask!.every(name => reply.actions.fields.includes(name))) {
      outcome.value = 'denied'; reviewedItem.value = undefined; actions.value = undefined;
      throw Error('This fact is no longer available for editing.');
    }
    if (!props.item && parents.value.length === 1) selectedParent.value = key(parents.value[0]!.target);
  } catch (e) { error.value = String(e); } finally {
    preparing.value = false;
  }
});
function reviewedChanges() {
  const selected = fields.value.filter(f => modes[f.id] !== 'preserve').map(f => typedField(f.id, f.type, values[f.id] ?? '', modes[f.id] === 'clear'));
  for (const name of equipmentFields.value) { const value = equipmentChanges[name]; if (value) selected.push(value); }
  return focused.value ? selected.filter(value => !WireField.equals(value, reviewedItem.value?.fields.find(f => f.name === value.name))) : selected;
}
const delta = computed(() => { try { return reviewedChanges(); } catch { return []; } });
async function reviewCurrent() {
  if (!props.item?.target || !props.fieldMask?.[0]) return;
  preparing.value = true; error.value = '';
  try {
    const working = model.WorkingContext.create({ ...editWorking, expected: undefined });
    const current = await call({ oneofKind: 'read', read: model.Read.create({ target: props.item.target, acceptedField: props.fieldMask[0] }) }, working);
    const prepared = await call({ oneofKind: 'prepare', prepare: model.Prepare.create({ view: props.view }) }, working);
    if (!mounted) return;
    if (current.oneofKind !== 'detail' || !current.detail.item || prepared.oneofKind !== 'actions') throw Error('The current fact could not be read.');
    if (!prepared.actions.fields.includes(props.fieldMask[0])) { outcome.value = 'denied'; throw Error('This fact is no longer available for editing.'); }
    reviewedItem.value = current.detail.item; actions.value = prepared.actions; outcome.value = 'reviewed';
  } catch (e) {
    const code = e instanceof Error && 'code' in e ? e.code : undefined;
    reviewedItem.value = undefined;
    if (outcome.value === 'denied' || code === 'PERMISSION_DENIED' || code === 'NOT_FOUND' || code === 'ABORTED') { outcome.value = 'denied'; actions.value = undefined; }
    error.value = String(e);
  }
  finally { preparing.value = false; }
}
async function save(next = false) {
  if(next&&!form.value?.reportValidity())return;
  let dispatched = false;
  busy.value = true; error.value = '';
  try {
    if (known.value && reviewedAddress.value && reviewedWorking.value) {
      const reply = await call({ oneofKind: 'address', address: reviewedAddress.value }, reviewedWorking.value);
      if (reply.oneofKind !== 'mutation') throw Error('Unexpected address response');
      emit('saved', props.item ? undefined : reply.mutation.targets[0]); return;
    }
    if (kind.value === undefined || !actions.value || (!props.item && !choice.value)) throw Error('Choose a resource and its actual parent');
    if(equipmentFields.value.some(name=>equipmentValidity[name]===false))throw Error('Correct the equipment input before saving.');
    if (focused.value && ['conflict', 'uncertain', 'denied'].includes(outcome.value)) throw Error('Read and review the current fact before another save.');
    const changes = reviewedChanges();
    if (placementKind.value && !placementReady.value) throw Error('Choose a listed placement or clear the search to leave it unknown');
    if (props.item && !changes.length && !known.value) { if (focused.value) emit('recheck'); else emit('saved'); return; }
    const working = model.WorkingContext.create({ ...(focused.value ? editWorking : props.working), focus: props.working.focus && placement.value?.target ? placement.value.target : props.working.focus, expected: placementActions.value?.revision ?? actions.value.revision });
    if (props.receipt) {
      if (kind.value !== AssetKind.DEVICE || choice.value?.parent) throw Error('Choose the permitted unplaced device context.');
      await props.receipt.submit(changes, working, next);
      return;
    }
    const parent: AssetRef | undefined = props.item ? undefined : choice.value?.parent?.target;
    const addresses: model.InitialAddress[] = [];
    if (!props.item && [AssetKind.DEVICE, AssetKind.VIRTUAL_MACHINE, AssetKind.SOFTWARE_INSTALLATION].includes(kind.value)) {
      for (const value of initialAddresses.value) {
        if (!value.address.trim() && value.vlanTag === undefined) continue;
        if (!value.address.trim()) throw Error('Enter an IP address when specifying a VLAN tag');
        const parts = value.address.trim().split('/');
        if (parts.length > 2 || (parts.length === 2 && !/^\d{1,3}$/.test(parts[1]!))) throw Error('Enter a valid IP address and optional prefix length');
        addresses.push(model.InitialAddress.create({ address: parts[0], prefixLength: parts.length === 2 ? Number(parts[1]) : value.prefixLength, vlanTag: value.vlanTag, interfaceName: value.interfaceName }));
      }
    }
    let command: model.Request['command'];
    if (known.value) {
      const extra = prefixEditor.value?.request() ?? {};
      const address = model.Address.create({ ...extra, target: props.item?.target, expected: props.item?.revision, parent });
      for (const change of changes) {
        if (change.name === 'address') address.address = values.address;
        else if (change.name === 'prefix_length') { if (modes.prefix_length === 'clear') address.clearPrefixLength = true; else address.prefixLength = Number(values.prefix_length); }
        else if (change.name === 'zone') { if (modes.zone === 'clear') address.clearZone = true; else address.zone = values.zone; }
        else if (change.name === 'broadcast') { if (modes.broadcast === 'clear') address.clearBroadcast = true; else address.broadcast = values.broadcast; }
        else if (change.name === 'meaning') address.meaning = model.AddressMeaning[values.meaning!.replace('ADDRESS_MEANING_','') as keyof typeof model.AddressMeaning] as model.AddressMeaning;
        else address.changes.push(change);
      }
      if (address.prefix && (working.focus || working.roots.length)) {
        const references = [...working.roots, parent, props.item?.target, address.prefix].filter((target): target is AssetRef => !!target);
        working.roots = [...new Map(references.map(target => [key(target), target])).values()];
        working.focus = undefined;
      }
      const refreshed = await call({ oneofKind: 'prepare', prepare: model.Prepare.create({ view: props.view }) }, model.WorkingContext.create({ ...working, expected: undefined }));
      if (refreshed.oneofKind !== 'actions') throw Error('The current address context is unavailable');
      working.expected = refreshed.actions.revision;
      const preview = await call({ oneofKind: 'previewAddress', previewAddress: address }, working);
      if (preview.oneofKind !== 'addressReview') throw Error('Address review is unavailable');
      addressReview.value = preview.addressReview; reviewedAddress.value = address;
      reviewedWorking.value = model.WorkingContext.create({ ...working, expected: preview.addressReview.revision });
      return;
    } else command = addresses.length ? { oneofKind: 'create', create: model.Create.create({ kind: kind.value, changes, parent, addresses }) } : { oneofKind: 'write', write: model.Write.create({ target: props.item?.target, expected: reviewedItem.value?.revision, kind: kind.value, changes, parent }) };
    dispatched = true;
    const reply = await call(command, working);
    if (!mounted) return;
    if (reply.oneofKind !== 'mutation') throw Error('Unexpected resource response');
    emit('saved', props.item ? undefined : reply.mutation.targets[0]);
  } catch (e) {
    error.value = String(e);
    if (focused.value && dispatched) {
      const code = e instanceof Error && 'code' in e ? e.code : undefined;
      if (code === 'ABORTED' || code === 'FAILED_PRECONDITION') outcome.value = 'conflict';
      else if (code === 'PERMISSION_DENIED' || code === 'NOT_FOUND') { outcome.value = 'denied'; reviewedItem.value = undefined; actions.value = undefined; }
      else if (!['INVALID_ARGUMENT', 'RESOURCE_EXHAUSTED'].includes(String(code))) { outcome.value = 'uncertain'; uncertain.value = true; }
    }
  } finally { busy.value = false; }
}
</script>
<template><component :is="inline ? 'section' : Dialog" :title="receipt ? 'Create received device' : focused ? 'Clarify recorded fact' : item ? 'Edit resource' : known ? 'Record known address' : 'Create resource'" :dismissible="!busy" @close="emit('close')"><form ref="form" aria-label="Resource editor" @submit.prevent="save()">
<p v-if="question">{{ question }}. This is the question, not the value to record.</p>
<section v-if="focused && reviewedItem" aria-label="Review fact change"><p>{{ input(reviewedItem.fields.find(f=>f.name==='name')) || 'Device' }}</p><p v-for="name in fieldMask" :key="name">Currently recorded {{ name.replaceAll('_',' ') }}: {{ display(reviewedItem.fields.find(f=>f.name===name)) || 'Unknown' }}<template v-if="delta.find(f=>f.name===name)"> → {{ display(delta.find(f=>f.name===name)) }}</template></p></section>
<p v-if="uncertain" role="status">The write outcome is unknown. Reading the current fact does not establish whether this attempt saved it. No write is retried automatically.</p>
<button v-if="focused && ['conflict','uncertain'].includes(outcome)" type="button" :disabled="preparing" @click="reviewCurrent">Read and review current fact</button>
<p v-if="preparing" role="status">Checking available actions…</p>
<template v-if="(!preparing || focused) && !addressReview">
<p v-if="known" class="hint">Record what you know. Network, VRF and mask may remain unknown. This does not reserve an address.</p>
<p v-else-if="!focused" class="hint">This action changes accepted inventory in the selected context.</p>
<p v-if="kind === AssetKind.CONTAINER" class="hint">Container IDs belong to an actual runtime. Link that runtime in a broader scope; a replacement gets a new container identity.</p>
<p v-if="kind === AssetKind.WORKLOAD" class="hint">Keep selected launch references if known. Compose labels identify grouping, not a verified owner or the original launch command.</p>
<p v-if="item && !focused" class="context-chip">Editing {{ input(item.fields.find(f => f.name === 'name')) || item.target?.assetId.slice(-8) }}</p>
<p v-else-if="!item && parents.length === 1" class="context-chip">{{ kind === AssetKind.VIRTUAL_MACHINE && parents[0]!.kind === AssetKind.SOFTWARE_INSTALLATION ? 'Hypervisor: ' : hypervisor && parents[0]!.kind === AssetKind.DEVICE ? 'Server: ' : 'Inside ' }}{{ parentLabel(parents[0]!) }}</p>
<label v-else-if="!item && parents.length" for="creation-parent">Create inside</label>
<select v-if="!item && parents.length > 1" id="creation-parent" v-model="selectedParent" required><option value="" disabled>Choose the actual parent</option><option v-for="p in parents" :key="key(p.target)" :value="key(p.target)">{{ parentLabel(p) }} · {{ label(p.kind) }}</option></select>
<p v-if="!item && choice?.parent" class="hint">Review ownership in the new resource's responsibility card. Owned-component links can supply inheritance; catalog and runtime placement do not assign a workload's owner.</p>
<label v-if="!focused" for="resource-kind">Resource kind</label>
<select v-show="!focused" id="resource-kind" v-model="resourceType" :disabled="!!item || busy || !!receipt" required>
<option v-if="item" :value="item.kind">{{ label(item.kind) }}</option>
<option v-else-if="!choices.length" disabled value="">No available action in this context</option>
<template v-for="c in item ? [] : choices" :key="c.kind">
<option v-if="c.kind !== AssetKind.SOFTWARE_INSTALLATION || view !== model.View.VIRTUALIZATION" :value="String(c.kind)">{{ label(c.kind) }}</option>
<option v-if="c.kind === AssetKind.SOFTWARE_INSTALLATION && c.fields.includes('virtualization_role')" value="hypervisor">Hypervisor (OS installation)</option>
</template>
</select>
<PlacementPicker v-if="placementKind" :kind="placementKind" :roots="placementRoots" @select="selectPlacement" @ready="placementReady = $event" />
<p v-if="placementBusy" role="status">Checking placement permissions…</p>
<ObserveHypervisor v-if="!item && kind === AssetKind.SOFTWARE_INSTALLATION" :current-name="values.name" @use="useObservation" />
<InitialAddresses v-if="!receipt && !item && kind !== undefined && [AssetKind.DEVICE,AssetKind.VIRTUAL_MACHINE,AssetKind.SOFTWARE_INSTALLATION].includes(kind)" v-model="initialAddresses" />
<button v-if="!item && kind === AssetKind.SOFTWARE_INSTALLATION && fields.some(f => f.id === 'virtualization_role')" type="button" @click="useProxmox">Use Proxmox VE defaults</button>
<details v-if="equipmentFields.length" :open="focused"><summary>Equipment knowledge</summary><p v-if="!focused">Record only what is known. Manufacturer, generation and model are separate facts. Refinement keeps this device's identity.</p>
<EquipmentField v-for="name in selectedEquipment.filter(f=>equipmentFields.includes(f))" :key="name" :name="name" :focused="focused" :disabled="busy" :value="equipmentChanges[name] ?? item?.fields.find(f=>f.name===name)" @change="equipmentChanges[name]=$event" @valid="equipmentValidity[name]=$event" />
<template v-if="!focused"><label>Add equipment fact<select v-model="equipmentToAdd" aria-label="Add equipment fact"><option value="">Choose a fact to record</option><option v-for="name in equipmentFields.filter(f=>!selectedEquipment.includes(f))" :key="name" :value="name">{{ name.replaceAll('_',' ') }}</option></select></label><button type="button" :disabled="!equipmentToAdd" @click="addEquipment">Add selected fact</button></template>
</details>
<button v-if="known" type="button" :aria-expanded="more" @click="more = !more">{{ more ? 'Hide additional facts' : 'More known details' }}</button>
<template v-for="f in fields" :key="f.id"><div v-if="!f.advanced || more" class="field-edit">
<label :for="'value-'+f.id">{{ f.name }}</label>
<select v-if="f.id !== 'virtualization_role' && (item || !['name','address','description'].includes(f.id))" :value="focused && modes[f.id] === 'preserve' ? 'set' : modes[f.id]" :disabled="busy" :aria-label="f.name+' change'" @change="modes[f.id]=($event.target as HTMLSelectElement).value"><option v-if="!focused" value="preserve">{{ item ? 'Preserve' : 'Leave unknown' }}</option><option value="set">Set</option><option v-if="!['name','address','meaning'].includes(f.id) || (f.id === 'name' && kind === AssetKind.PREFIX)" value="clear">Clear explicitly</option></select>
<select v-if="f.id === 'virtualization_role'" :id="'value-'+f.id" v-model="roleInput" class="role-selector"><option v-if="item" value="preserve">Keep current value</option><option value="unknown">{{ kind === AssetKind.DEVICE ? 'Clear retired device role' : 'Unknown' }}</option><template v-if="kind === AssetKind.SOFTWARE_INSTALLATION"><option v-for="c in virtualizationChoices" :key="c.value" :value="c.value">{{ c.label }}</option></template></select>
<select v-else-if="f.type === model.FieldType.BOOLEAN" :id="'value-'+f.id" v-model="values[f.id]" :disabled="modes[f.id] !== 'set'" :required="modes[f.id] === 'set'"><option disabled value="">Choose true or false</option><option value="true">True</option><option value="false">False</option></select>
<select v-else-if="f.choices.length" :id="'value-'+f.id" v-model="values[f.id]" :disabled="modes[f.id] !== 'set'" :required="modes[f.id] === 'set'"><option disabled value="">Choose a value</option><option v-for="value in f.choices" :key="value" :value="value">{{ f.id === 'meaning' ? value.replace('ADDRESS_MEANING_', '').toLowerCase() : f.id === 'virtualization_role' ? virtualizationChoices.find(c => c.value === value)?.label : softwareChoiceLabel(f.id, value) }}</option></select>
<textarea v-else-if="f.id === 'description' || f.id === 'compose_files'" :id="'value-'+f.id" v-model="values[f.id]" rows="2" maxlength="2048" :disabled="busy || modes[f.id] === 'clear'" @input="modes[f.id] = !item && values[f.id] === '' ? 'preserve' : 'set'"></textarea>
<!-- Integer text goes directly to BigInt; number inputs round valid int64 values. -->
<input v-else :id="'value-'+f.id" v-model="values[f.id]" :type="f.type === model.FieldType.DATE ? 'date' : 'text'" :inputmode="f.type === model.FieldType.INTEGER ? 'numeric' : undefined" :disabled="modes[f.id] !== 'set'" :required="modes[f.id] === 'set' && ['name','address'].includes(f.id)" :maxlength="f.id === 'name' ? 160 : f.id === 'address' ? 192 : 2048" @input="modes[f.id] = 'set'">
<p v-if="f.id === 'virtualization_role'" class="hint">{{ kind === AssetKind.DEVICE ? 'This retired field can only be cleared. Set virtualization roles on OS installations.' : 'Requires installed and enabled Hyper-V (Windows) or virtualization components (Linux). Leave unknown when unverified.' }}</p>
</div></template>
</template>
<AddressPrefix v-if="known && !preparing" :key="key(item?.target)+'/'+key(choice?.parent?.target)+'/'+key(initialPrefix?.target)" v-show="!addressReview" ref="prefixEditor" v-model="prefixChoice" :address="values.address ?? ''" :prefix-length="submittedMask" :editing="!!item" :initial-prefix="initialPrefix" :working="working" />
<section v-if="addressReview" aria-label="Review known address"><h3>Review known address</h3><dl>
<dt>Address</dt><dd>{{ addressReview.address }}</dd>
<dt>Assignment mask</dt><dd>{{ reviewedAddress?.clearPrefixLength ? 'Clear explicitly' : addressReview.prefixLength === undefined ? 'Unknown' : '/'+addressReview.prefixLength }}</dd>
<template v-if="reviewedMeaning"><dt>Address meaning</dt><dd>{{ reviewedMeaning }}</dd></template>
<template v-if="addressReview.zone || reviewedAddress?.clearZone"><dt>Local IPv6 zone</dt><dd>{{ reviewedAddress?.clearZone ? 'Clear explicitly' : addressReview.zone }}</dd></template>
<template v-if="addressReview.prefixCidr"><dt>{{ reviewedAddress?.createPrefix ? 'New prefix, namespace unknown' : 'Selected prefix' }}</dt><dd>{{ addressReview.prefixCidr }}</dd></template>
<template v-if="reviewedAddress?.clearPrefix"><dt>Prefix relationship</dt><dd>Remove the relationship only</dd></template>
<template v-if="addressReview.broadcast || reviewedAddress?.clearBroadcast"><dt>Explicit endpoint broadcast</dt><dd>{{ reviewedAddress?.clearBroadcast ? 'Clear explicitly' : addressReview.broadcast }}</dd></template>
<template v-for="change in reviewedAddress?.changes" :key="change.name"><dt>{{ fields.find(f => f.id === change.name)?.name ?? change.name }}</dt><dd>{{ display(change) }}</dd></template>
</dl><p>No allocation, interface assignment, route or provider effect is implied.</p><button type="button" @click="addressReview = undefined; reviewedAddress = undefined; reviewedWorking = undefined">Back to input</button></section>
<p v-if="addressReview?.zone" class="hint">Host-local zone: {{ addressReview.zone }}</p>
<p v-if="addressReview && reviewedAddress?.clearBroadcast">The recorded endpoint broadcast will be cleared.</p>
<p v-if="addressReview && reviewedAddress?.clearPrefix">The prefix relationship will be removed; the prefix remains.</p>
<p v-if="addressReview && reviewedAddress?.prefix" class="hint">Selected prefix reference: {{ reviewedAddress.prefix.assetId }}/{{ reviewedAddress.prefix.incarnation }}</p>
<p v-if="addressReview && reviewedAddress?.createPrefix && reviewedAddress.prefixName" class="hint">New prefix name: {{ reviewedAddress.prefixName }}</p>
<p v-if="addressReview && derivedBroadcast" class="hint">Calculated subnet broadcast: {{ derivedBroadcast }}. This is a display calculation, not a recorded endpoint setting.</p>
<p v-else-if="addressReview?.address?.includes(':')" class="hint">IPv6 has no broadcast address.</p>
<p v-else-if="addressReview?.prefixLength === 31 || addressReview?.prefixLength === 32" class="hint">This mask has no directed broadcast. A /31 may describe a point-to-point link.</p>
<p v-if="error" role="alert">{{ error }}</p><footer><button type="button" :disabled="busy" @click="emit('close')">Cancel</button><button v-if="focused" type="button" :disabled="busy || preparing" @click="emit('recheck')">Recheck without saving</button><button class="primary" :disabled="busy || preparing || placementBusy || (!!placementKind && !placementReady) || (!item && !choice) || (focused && ['conflict','uncertain','denied'].includes(outcome))">{{ busy ? known && !addressReview ? 'Preparing review…' : 'Saving…' : receipt ? 'Record received device' : focused ? 'Save and recheck' : known ? addressReview ? 'Save known address' : 'Review known address' : 'Save resource' }}</button><button v-if="receipt" type="button" :disabled="busy || preparing || !choice" @click="save(true)">Record and next</button></footer>
</form></component></template>
<style scoped>
.field-edit > .hint, .field-edit > .role-selector, .field-edit > textarea { grid-column:1/-1; }
.field-edit > .hint { margin:0; }
</style>
