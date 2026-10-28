<script setup lang="ts">
import { computed, onMounted, reactive, ref, watch } from 'vue';
import Dialog from './Dialog.vue';
import PlacementPicker from './PlacementSearch.vue';
import InitialAddresses from './InitialAddresses.vue';
import ObserveHypervisor from './ObserveHypervisor.vue';
import { softwareFields, softwareChoiceLabel } from './software-fields';
import { call, typedField, input, label, key, model, AssetKind, type AssetRef } from './api';

const props = defineProps<{ item?: model.Item; definitions: model.Definition[]; initialKind?: AssetKind; initialHypervisor?: boolean; working: model.WorkingContext; view: model.View }>();
const emit = defineEmits<{ close: []; saved: [target?: AssetRef] }>();
const kind = ref<number | undefined>(props.item?.kind ?? props.initialKind);
const actions = ref<model.Actions>();
const selectedParent = ref('');
const hypervisor = ref(!!props.initialHypervisor || (!props.item && props.view === model.View.VIRTUALIZATION && props.initialKind === AssetKind.SOFTWARE_INSTALLATION));
const placement = ref<model.Item>(); const placementReady = ref(true); const placementBusy = ref(false);
const placementActions = ref<model.Actions>();
const initialAddresses = ref([model.InitialAddress.create()]);
const values = reactive<Record<string, string>>({});
const modes = reactive<Record<string, string>>({});
const error = ref(''); const busy = ref(false); const preparing = ref(true); const more = ref(false);
type EditorField = { id: string; name: string; type: model.FieldType; choices: string[]; advanced: boolean };
const parents = computed(() => [...new Map(actions.value?.creations.flatMap(c => c.parent?.target ? [[key(c.parent.target), c.parent] as const] : [])).values()]);
const choices = computed(() => actions.value?.creations.filter(c => key(c.parent?.target) === selectedParent.value) ?? []);
const baseChoice = computed(() => choices.value.find(c => c.kind === kind.value));
const choice = computed(() => placement.value ? placementActions.value?.creations.find(c => c.kind === kind.value && key(c.parent?.target) === key(placement.value?.target)) : baseChoice.value);
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
    { id: 'description', name: 'Description', type: model.FieldType.TEXT, choices: [], advanced: true },
  ] : [
    { id: 'name', name: 'Name', type: model.FieldType.TEXT, choices: [], advanced: false },
    { id: 'description', name: 'Description', type: model.FieldType.TEXT, choices: [], advanced: false },
    { id: 'serial', name: 'Serial number', type: model.FieldType.TEXT, choices: [], advanced: false },
    { id: 'virtualization_role', name: 'Virtualization role', type: model.FieldType.CHOICE, choices: virtualizationChoices.map(v => v.value), advanced: false },
  ];
  const custom = props.definitions.filter(d => !d.retired && kind.value !== undefined && d.kinds.includes(kind.value))
    .map(d => ({ id: d.id, name: d.name, type: d.type, choices: d.choices, advanced: false }));
  return [...builtins, ...softwareFields(allowed), ...custom].filter(f => allowed.includes(f.id));
});
watch(fields, available => {
  for (const f of available) {
    const first = !(f.id in values);
    if (!(f.id in values)) values[f.id] = input(props.item?.fields.find(v => v.name === f.id));
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
  try {
    const working = model.WorkingContext.create({ ...props.working, focus: props.item?.target ?? props.working.focus, expected: undefined });
    const reply = await call({ oneofKind: 'prepare', prepare: model.Prepare.create({ view: props.view }) }, working);
    if (reply.oneofKind !== 'actions') throw Error('Creation context is unavailable');
    actions.value = reply.actions;
    if (!props.item && parents.value.length === 1) selectedParent.value = key(parents.value[0]!.target);
  } catch (e) { error.value = String(e); } finally { preparing.value = false; }
});
async function save() {
  busy.value = true; error.value = '';
  try {
    if (kind.value === undefined || !actions.value || (!props.item && !choice.value)) throw Error('Choose a resource and its actual parent');
    const changes = fields.value.filter(f => modes[f.id] !== 'preserve').map(f => typedField(f.id, f.type, values[f.id] ?? '', modes[f.id] === 'clear'));
    if (placementKind.value && !placementReady.value) throw Error('Choose a listed placement or clear the search to leave it unknown');
    if (props.item && !changes.length) { emit('saved'); return; }
    const working = model.WorkingContext.create({ ...props.working, focus: props.working.focus && placement.value?.target ? placement.value.target : props.working.focus, expected: placementActions.value?.revision ?? actions.value.revision });
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
    const reply = await call(addresses.length ? { oneofKind: 'create', create: model.Create.create({ kind: kind.value, changes, parent, addresses }) } : { oneofKind: 'write', write: model.Write.create({ target: props.item?.target, expected: props.item?.revision, kind: kind.value, changes, parent }) }, working);
    if (reply.oneofKind !== 'mutation') throw Error('Unexpected resource response');
    emit('saved', props.item ? undefined : reply.mutation.targets[0]);
  } catch (e) { error.value = String(e); } finally { busy.value = false; }
}
</script>
<template><Dialog :title="item ? 'Edit resource' : known ? 'Record known address' : 'Create resource'" @close="emit('close')"><form @submit.prevent="save">
<p v-if="preparing" role="status">Checking available actions…</p>
<template v-else>
<p v-if="known" class="hint">Record what you know. Network, VRF and mask may remain unknown. This does not reserve an address.</p>
<p v-else class="hint">This action changes accepted inventory in the selected context.</p>
<p v-if="kind === AssetKind.CONTAINER" class="hint">Container IDs belong to an actual runtime. Link that runtime in a broader scope; a replacement gets a new container identity.</p>
<p v-if="kind === AssetKind.WORKLOAD" class="hint">Keep selected launch references if known. Compose labels identify grouping, not a verified owner or the original launch command.</p>
<p v-if="item" class="context-chip">Editing {{ input(item.fields.find(f => f.name === 'name')) || item.target?.assetId.slice(-8) }}</p>
<p v-else-if="parents.length === 1" class="context-chip">{{ kind === AssetKind.VIRTUAL_MACHINE && parents[0]!.kind === AssetKind.SOFTWARE_INSTALLATION ? 'Hypervisor: ' : hypervisor && parents[0]!.kind === AssetKind.DEVICE ? 'Server: ' : 'Inside ' }}{{ parentLabel(parents[0]!) }}</p>
<label v-else-if="parents.length" for="creation-parent">Create inside</label>
<select v-if="!item && parents.length > 1" id="creation-parent" v-model="selectedParent" required><option value="" disabled>Choose the actual parent</option><option v-for="p in parents" :key="key(p.target)" :value="key(p.target)">{{ parentLabel(p) }} · {{ label(p.kind) }}</option></select>
<p v-if="!item && choice?.parent" class="hint">Review ownership in the new resource's responsibility card. Owned-component links can supply inheritance; catalog and runtime placement do not assign a workload's owner.</p>
<label for="resource-kind">Resource kind</label>
<select id="resource-kind" v-model="resourceType" :disabled="!!item || busy" required>
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
<InitialAddresses v-if="!item && kind !== undefined && [AssetKind.DEVICE,AssetKind.VIRTUAL_MACHINE,AssetKind.SOFTWARE_INSTALLATION].includes(kind)" v-model="initialAddresses" />
<button v-if="!item && kind === AssetKind.SOFTWARE_INSTALLATION && fields.some(f => f.id === 'virtualization_role')" type="button" @click="useProxmox">Use Proxmox VE defaults</button>
<button v-if="known" type="button" :aria-expanded="more" @click="more = !more">{{ more ? 'Hide additional facts' : 'More known details' }}</button>
<template v-for="f in fields" :key="f.id"><div v-if="!f.advanced || more" class="field-edit">
<label :for="'value-'+f.id">{{ f.name }}</label>
<select v-if="f.id !== 'virtualization_role' && (item || !['name','address','description'].includes(f.id))" v-model="modes[f.id]" :aria-label="f.name+' change'"><option value="preserve">{{ item ? 'Preserve' : 'Leave unknown' }}</option><option value="set">Set</option><option v-if="!['name','address','meaning'].includes(f.id)" value="clear">Clear explicitly</option></select>
<select v-if="f.id === 'virtualization_role'" :id="'value-'+f.id" v-model="roleInput" class="role-selector"><option v-if="item" value="preserve">Keep current value</option><option value="unknown">{{ kind === AssetKind.DEVICE ? 'Clear retired device role' : 'Unknown' }}</option><template v-if="kind === AssetKind.SOFTWARE_INSTALLATION"><option v-for="c in virtualizationChoices" :key="c.value" :value="c.value">{{ c.label }}</option></template></select>
<select v-else-if="f.type === model.FieldType.BOOLEAN" :id="'value-'+f.id" v-model="values[f.id]" :disabled="modes[f.id] !== 'set'" :required="modes[f.id] === 'set'"><option disabled value="">Choose true or false</option><option value="true">True</option><option value="false">False</option></select>
<select v-else-if="f.choices.length" :id="'value-'+f.id" v-model="values[f.id]" :disabled="modes[f.id] !== 'set'" :required="modes[f.id] === 'set'"><option disabled value="">Choose a value</option><option v-for="value in f.choices" :key="value" :value="value">{{ f.id === 'meaning' ? value.replace('ADDRESS_MEANING_', '').toLowerCase() : f.id === 'virtualization_role' ? virtualizationChoices.find(c => c.value === value)?.label : softwareChoiceLabel(f.id, value) }}</option></select>
<textarea v-else-if="f.id === 'description' || f.id === 'compose_files'" :id="'value-'+f.id" v-model="values[f.id]" rows="2" maxlength="2048" :disabled="modes[f.id] === 'clear'" @input="modes[f.id] = !item && values[f.id] === '' ? 'preserve' : 'set'"></textarea>
<!-- Integer text goes directly to BigInt; number inputs round valid int64 values. -->
<input v-else :id="'value-'+f.id" v-model="values[f.id]" :type="f.type === model.FieldType.DATE ? 'date' : 'text'" :inputmode="f.type === model.FieldType.INTEGER ? 'numeric' : undefined" :disabled="modes[f.id] !== 'set'" :required="modes[f.id] === 'set' && ['name','address'].includes(f.id)" :maxlength="f.id === 'name' ? 160 : f.id === 'address' ? 64 : 2048" @input="modes[f.id] = 'set'">
<p v-if="f.id === 'virtualization_role'" class="hint">{{ kind === AssetKind.DEVICE ? 'This retired field can only be cleared. Set virtualization roles on OS installations.' : 'Requires installed and enabled Hyper-V (Windows) or virtualization components (Linux). Leave unknown when unverified.' }}</p>
</div></template>
</template>
<p v-if="error" role="alert">{{ error }}</p><footer><button type="button" @click="emit('close')">Cancel</button><button class="primary" :disabled="busy || preparing || placementBusy || (!!placementKind && !placementReady) || (!item && !choice)">{{ busy ? 'Saving…' : known ? 'Save known address' : 'Save resource' }}</button></footer>
</form></Dialog></template>
<style scoped>
.field-edit > .hint, .field-edit > .role-selector, .field-edit > textarea { grid-column:1/-1; }
.field-edit > .hint { margin:0; }
</style>
