<script setup lang="ts">
import { computed, ref, watch } from 'vue';
import { call, model } from './api';
const props = defineProps<{ currentName?: string }>();
const emit = defineEmits<{ use: [value: { name?: string; addresses: model.InitialAddress[] }] }>();
const endpoint = ref(''); const method = ref('none'); const username = ref(''); const password = ref('');
const tokenId = ref(''); const tokenSecret = ref(''); const busy = ref(false); const error = ref('');
const preview = ref<model.ObservationPreview>(); const nodeIndex = ref(''); const includeName = ref(false); const selected = ref<number[]>([]);
const node = computed(() => nodeIndex.value === '' ? undefined : preview.value?.nodes[Number(nodeIndex.value)]);
watch(endpoint, () => { preview.value = undefined; error.value = ''; });
watch(() => props.currentName, () => { includeName.value = false; });
watch(node, value => { selected.value = value && value.addresses.length <= 16 ? value.addresses.map((_,i) => i) : []; });
async function observe() {
  if (busy.value || !endpoint.value.trim()) return;
  busy.value = true; error.value = ''; preview.value = undefined;
  try {
    const reply = await call({ oneofKind: 'observeProxmox', observeProxmox: model.ObserveProxmox.create({
      endpoint: endpoint.value.trim(), username: method.value === 'login' ? username.value : '', password: method.value === 'login' ? password.value : '',
      tokenId: method.value === 'token' ? tokenId.value : '', tokenSecret: method.value === 'token' ? tokenSecret.value : '',
    }) });
    if (reply.oneofKind !== 'observation') throw Error('The observer returned no reviewable result');
    preview.value = reply.observation; nodeIndex.value = reply.observation.nodes.length === 1 ? '0' : '';
    includeName.value = props.currentName === undefined || props.currentName.trim() === '';
  } catch (e) { error.value = String(e); }
  finally { busy.value = false; username.value = ''; password.value = ''; tokenId.value = ''; tokenSecret.value = ''; }
}
function apply() {
  if (!node.value || selected.value.length > 16) return;
  emit('use', { name: includeName.value ? node.value.name : undefined, addresses: node.value.addresses.filter((_,i) => selected.value.includes(i)) });
  preview.value = undefined;
}
function memory(bytes: bigint) { const unit=1073741824n; return `${bytes/unit}.${String((bytes%unit)*10n/unit)} GiB`; }
</script>
<template><section class="observer" aria-label="Hypervisor observation">
<div class="observer-heading"><h3>Read from Proxmox VE</h3><button type="button" :disabled="busy || !endpoint.trim()" @click="observe">{{ busy ? 'Observing…' : 'Observe' }}</button></div>
<label for="observe-endpoint">API endpoint<input id="observe-endpoint" v-model="endpoint" inputmode="url" placeholder="https://hypervisor.example:8006" autocomplete="off" maxlength="2048" :disabled="busy" @keydown.enter.prevent="observe"></label>
<details><summary>Optional one-time credentials</summary>
<label for="observe-method">Authentication</label><select id="observe-method" v-model="method" :disabled="busy"><option value="none">No credentials</option><option value="login">Login and password</option><option value="token">API token</option></select>
<template v-if="method === 'login'"><label for="observe-user">Login including realm<input id="observe-user" v-model="username" placeholder="user@pve" autocomplete="off" maxlength="1024" :disabled="busy"></label><label for="observe-password">Password<input id="observe-password" v-model="password" type="password" autocomplete="off" maxlength="1024" :disabled="busy"></label></template>
<template v-if="method === 'token'"><label for="observe-token-id">Token ID<input id="observe-token-id" v-model="tokenId" placeholder="user@pve!token" autocomplete="off" maxlength="1024" :disabled="busy"></label><label for="observe-token-secret">Token secret<input id="observe-token-secret" v-model="tokenSecret" type="password" autocomplete="off" maxlength="1024" :disabled="busy"></label></template>
<p class="hint">Used for this read only, then cleared. Credentials are not saved with inventory.</p>
</details>
<p v-if="error" role="alert">{{ error }}</p>
<div v-if="preview" class="observation-preview"><h3>Review observed values</h3><p>Proxmox VE {{ preview.version }}</p>
<label v-if="preview.nodes.length > 1" for="observe-node">Node<select id="observe-node" v-model="nodeIndex"><option value="" disabled>Choose the actual installation</option><option v-for="(n,i) in preview.nodes" :key="n.name" :value="String(i)">{{ n.name }}</option></select></label>
<p v-if="!preview.nodes.length">No nodes were returned by this account.</p>
<template v-if="node"><label class="check"><input v-model="includeName" type="checkbox">Use node name: {{ node.name }}</label><p v-if="node.cpuModel || node.memoryBytes !== undefined">{{ node.cpuModel }}<template v-if="node.memoryBytes !== undefined"> · {{ memory(node.memoryBytes) }}</template></p>
<p v-if="node.addresses.length > 16" class="hint">Choose up to 16 addresses for this resource.</p><ul><li v-for="(address,i) in node.addresses" :key="i"><label class="check"><input v-model="selected" type="checkbox" :value="i" :disabled="!selected.includes(i) && selected.length >= 16">{{ address.address }}<template v-if="address.prefixLength !== undefined">/{{ address.prefixLength }}</template><small>{{ address.interfaceName }}<template v-if="address.vlanTag !== undefined"> · VLAN {{ address.vlanTag }}</template></small></label></li></ul>
<p v-for="limitation in node.limitations" :key="limitation" class="hint">{{ limitation }}</p>
<p class="hint">Use selected values as accepted inventory. This does not assert live health or merge a physical server.</p>
<button type="button" @click="apply">Use selected values</button>
</template></div>
</section></template>
<style scoped>
.observer { padding:12px;margin:16px 0;background:#f1f7f7;border:1px solid #dae4e7;border-radius:6px; }
.observer-heading { display:flex;align-items:center;justify-content:space-between;gap:8px;margin-bottom:10px; }
.observer-heading h3 { margin:0; }
.observer label:not(.check) { margin:10px 0; }
.observer input,.observer select { width:100%;min-width:0; }
.observer summary { cursor:pointer;min-height:44px;padding:10px 0; }
.observer input[type=checkbox] { width:18px;height:18px;flex-shrink:0;min-height:0; }
.observer .check { display:flex;flex-direction:row;align-items:center;gap:8px;min-height:44px;margin:0;flex-wrap:wrap;overflow-wrap:anywhere; }
.observer ul { list-style:none;margin:8px 0;padding:0;max-height:240px;overflow:auto; }
.observer small { color:#6e818b; }
.observation-preview { border-top:1px solid #dae4e7;padding-top:12px;margin-top:12px; }
.observation-preview p { overflow-wrap:anywhere; }
</style>
