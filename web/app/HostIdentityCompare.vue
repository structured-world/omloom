<script setup lang="ts">
import { ref, computed, onMounted } from 'vue';
import Dialog from './Dialog.vue';
import HostIdentityEvidence from './HostIdentityEvidence.vue';
import { call, model, display, label, type AssetRef } from './api';
const props = defineProps<{ left: AssetRef; right: AssetRef; working: model.WorkingContext }>();
const emit = defineEmits<{ close: [] }>();
const comparison = ref<model.HostIdentityComparison>();
const error = ref('');
const busy = ref(false);
const matches = computed(() => comparison.value?.left?.identity.filter(a => a.claim && comparison.value?.right?.identity.some(b => b.claim && a.claim?.kind === b.claim.kind && a.claim.namespace === b.claim.namespace && a.claim.digest === b.claim.digest)) ?? []);
async function load() {
  busy.value = true; error.value = ''; comparison.value = undefined;
  try {
    const response = await call({ oneofKind: 'compareHostIdentity', compareHostIdentity: model.CompareHostIdentity.create({ left: props.left, right: props.right }) }, model.WorkingContext.create({ ...props.working, expected: undefined }));
    if (response.oneofKind !== 'hostIdentityComparison') throw Error('Identity comparison is unavailable.');
    comparison.value = response.hostIdentityComparison;
  } catch (e) { error.value = String(e); }
  finally { busy.value = false; }
}
onMounted(load);
</script>
<template><Dialog title="Compare identity" @close="emit('close')">
<p>Compare hardware and installed operating systems separately. Different IPs do not prove different machines. Matching identifiers may be cloned, movable or reported by the same underlying source.</p>
<p v-if="busy" role="status">Loading the two selected resources…</p>
<p v-if="error" role="alert">{{ error }}</p>
<template v-if="comparison">
<p>Current comparison at revision {{ comparison.revision.toString() }}. This read changes no inventory, grants or connections.</p>
<p v-if="matches.length">Some identifiers match within the same layer and namespace. This supports review, not automatic merging or authenticated target verification.</p>
<p v-else>No matching identity claims are available in this comparison. Missing evidence does not prove that these are different machines.</p>
<div class="identity-pair"><section v-for="(subject, index) in [comparison.left, comparison.right]" :key="index" :aria-label="index === 0 ? 'First identity' : 'Second identity'">
<template v-if="subject?.item"><h3>{{ display(subject.item.fields.find(v => v.name === 'name')) }}</h3>
<p>{{ label(subject.item.kind) }}</p><dl><template v-for="field in subject.item.fields.filter(v => v.name !== 'name')" :key="field.name"><dt>{{ field.name.replaceAll('_', ' ') }}</dt><dd>{{ display(field) }}</dd></template></dl>
<HostIdentityEvidence :observations="subject.identity" />
<p v-if="!subject.identity.length">No identity evidence is available to this view. Do not infer hardware or installation continuity from the name.</p>
</template></section></div></template>
<footer><button @click="emit('close')">Back</button><button v-if="error" :disabled="busy" @click="load">Retry comparison</button></footer>
</Dialog></template>
<style scoped>
.identity-pair { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: 1rem; }
.identity-pair > section { min-width: 0; overflow-wrap: anywhere; }
@media (max-width: 700px) { .identity-pair { grid-template-columns: minmax(0, 1fr); } }
</style>
