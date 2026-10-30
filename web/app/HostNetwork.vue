<script setup lang="ts">
import { computed, ref, watch } from 'vue';
import { model, enumLabel, type AssetRef } from './api';
const props = defineProps<{ evidence: model.HostNetworkView }>();
const emit = defineEmits<{ open: [target: AssetRef | undefined] }>();
const serviceName = (target:AssetRef|undefined) => props.evidence.services.find(s=>s.target?.assetId===target?.assetId && s.target?.incarnation===target?.incarnation)?.name;
const section = ref('endpoints'); const term = ref(''); const page = ref(0);
const addressClass = (e:model.SocketBind) => e.bind === '*' ? 'all interfaces in this family' : `${enumLabel(model.AddressStability, e.addressStability)} · ${enumLabel(model.AddressState, e.addressState)}`;
const coverage = computed(() => props.evidence.coverage);
const allSockets = computed(() => props.evidence.observations.flatMap(o => o.value.oneofKind === 'socket' ? [{...o.value.socket, observedAt:o.observedAt}] : []));
const allRoutes = computed(() => props.evidence.observations.flatMap(o => o.value.oneofKind === 'route' ? [{...o.value.route, observedAt:o.observedAt}] : []));
const allRules = computed(() => props.evidence.observations.flatMap(o => o.value.oneofKind === 'rule' ? [{...o.value.rule, observedAt:o.observedAt}] : []));
const sockets = computed(() => allSockets.value.filter(e => `${serviceName(e.service)} ${e.protocol} ${e.bind} ${e.port}`.toLowerCase().includes(term.value.toLowerCase())));
const routes = computed(() => allRoutes.value.filter(r => `${r.destination} ${r.gateway} ${r.interface} ${r.family}`.toLowerCase().includes(term.value.toLowerCase())));
const rules = computed(() => allRules.value.filter(r => `${r.subject} ${r.action}`.toLowerCase().includes(term.value.toLowerCase())));
const total = computed(() => section.value === 'endpoints' ? sockets.value.length : section.value === 'routes' ? routes.value.length : rules.value.length);
const start = computed(() => page.value * 20);
function policy(kind:model.FirewallSetting_Kind) {
  const fact=props.evidence.observations.find(o=>o.value.oneofKind==='policy' && o.value.policy.kind===kind);
  return fact?.value.oneofKind==='policy' ? `${fact.value.policy.enabled?'Enabled':'Disabled'} · Observed ${new Date(Number(fact.observedAt)).toLocaleString()}` : 'Unknown';
}
function reset() { page.value = 0; }
watch(() => props.evidence, reset);
</script>
<template><section v-if="coverage" class="host-network" aria-label="Local network observations">
<h3>Local network observations</h3><p v-if="evidence.bindingUnresolved" class="hint">Legacy source attribution is unresolved. Original observation dates are preserved.</p><p class="hint">{{ coverage.source }} · Latest collection {{ new Date(Number(coverage.observedAt)).toLocaleString() }}. Each fact keeps its own observation date. Dated source evidence, not live reachability.</p>
<div class="actions"><button @click="section = 'endpoints'; reset()" :aria-pressed="section === 'endpoints'">Endpoints ({{ allSockets.length }})</button><button v-if="coverage.routeCoverage" @click="section = 'routes'; reset()" :aria-pressed="section === 'routes'">Routes ({{ allRoutes.length }})</button><button v-if="coverage.applicationCoverage" @click="section = 'firewall'; reset()" :aria-pressed="section === 'firewall'">Firewall ({{ allRules.length }})</button></div>
<label>Filter observations<input v-model="term" type="search" @input="reset" placeholder="Service, address, interface or application"></label>
<template v-if="section === 'endpoints'"><p class="hint">{{ coverage.socketCoverage }}</p><p class="hint">Host-local and temporary binds are omitted. Temporary addresses are never retained.</p><p>TCP LISTEN and unconnected UDP binds. UDP does not prove a server role. Effective firewall decision: unknown.</p><div class="table-scroll"><table><thead><tr><th>Service</th><th>Transport / family</th><th>Local bind</th><th>Observed</th></tr></thead><tbody><tr v-for="(e, i) in sockets.slice(start, start + 20)" :key="i"><td><button @click="emit('open', e.service)">{{ serviceName(e.service) }}</button></td><td>{{ e.protocol.toUpperCase() }} / {{ e.family }}</td><td class="mono">{{ e.bind.includes(':') ? '['+e.bind+']' : e.bind }}:{{ e.port }}<small> · {{ addressClass(e) }}</small></td><td>{{ new Date(Number(e.observedAt)).toLocaleString() }}</td></tr></tbody></table></div></template>
<template v-if="section === 'routes'"><p class="hint">{{ coverage.routeCoverage }}. Source notation is preserved. VPN routes do not establish ownership or upstream topology.</p><div class="table-scroll"><table><thead><tr><th>Destination</th><th>Gateway</th><th>Interface</th><th>Source flags / family</th><th>Observed</th></tr></thead><tbody><tr v-for="(r, i) in routes.slice(start, start + 20)" :key="i"><td class="mono">{{ r.destination }}<small v-if="r.linkLayer"> · link-layer / local host</small></td><td class="mono">{{ r.gateway }}</td><td>{{ r.interface }}</td><td>{{ r.flags }} / {{ r.family }}</td><td>{{ new Date(Number(r.observedAt)).toLocaleString() }}</td></tr></tbody></table></div></template>
<template v-if="section === 'firewall'"><p>Application firewall: {{ policy(model.FirewallSetting_Kind.ENABLED) }}. Block all: {{ policy(model.FirewallSetting_Kind.BLOCK_ALL) }}.</p><p>Allow built-in signed apps: {{ policy(model.FirewallSetting_Kind.ALLOW_BUILTIN_SIGNED) }}. Allow downloaded signed apps: {{ policy(model.FirewallSetting_Kind.ALLOW_DOWNLOADED_SIGNED) }}.</p><p class="hint">Application policy: {{ coverage.applicationCoverage }}</p><p role="status">Packet filter: {{ coverage.packetFilterCoverage }}. Effective per-port decision: unknown.</p><p class="hint">An app allowance is not a packet verdict. Rules are not matched to processes by basename.</p><div class="table-scroll"><table><thead><tr><th>Application identity reported by source</th><th>Action</th><th>Observed</th></tr></thead><tbody><tr v-for="(r, i) in rules.slice(start, start + 20)" :key="i"><td class="mono">{{ r.subject }}</td><td>{{ r.action }}</td><td>{{ new Date(Number(r.observedAt)).toLocaleString() }}</td></tr></tbody></table></div></template>
<p v-if="!total" class="hint">No matching rows in the supplied evidence. Check source coverage before interpreting absence.</p><footer class="pagination"><span>{{ total }} matching observations</span><div><button :disabled="page === 0" @click="page--">Previous observations</button><button :disabled="start + 20 >= total" @click="page++">Next observations</button></div></footer>
</section></template>
<style scoped>
.host-network { border-top: 1px solid var(--line, #dce2e8); padding-top: 16px; container-type: inline-size; }
.host-network > label:not(.check) { display: grid; gap: 6px; margin: 12px 0; }
.host-network .mono { overflow-wrap: anywhere; white-space: normal; color: #31444e; font-size: 12px; }
.host-network td { max-width: 360px; }
.host-network .actions { flex-wrap: wrap; }
.host-network td > button { white-space: nowrap; }
@container (max-width: 600px) {
  .host-network table, .host-network tbody { display: block; white-space: normal; }
  .host-network thead { display: none; }
  .host-network tbody tr { display: grid; grid-template-columns: minmax(0, 1fr); gap: 6px; padding: 12px 0; border-top: 1px solid #dce2e8; }
  .host-network tbody td, .host-network tbody td:first-child { grid-column: 1; grid-row: auto; max-width: none; padding: 0; border: 0; overflow-wrap: anywhere; }
  .host-network td > button { width: fit-content; max-width: 100%; white-space: normal; text-align: left; }
  .host-network .table-scroll { overflow: auto; }
}
</style>
