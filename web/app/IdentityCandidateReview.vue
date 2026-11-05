<script setup lang="ts">
import { computed } from 'vue';
import { model, display, key, label } from './api';
const props=defineProps<{finding:model.HostFinding;candidates:model.Item[]}>();
defineEmits<{confirm:[value:model.Item]}>();
const fields=computed(()=>[...new Set([...props.finding.fields.map(v=>v.name),...props.candidates.flatMap(v=>v.fields.map(v=>v.name))])]);
</script>
<template><section aria-label="Compare observation candidates">
<h4>Review the same subject layer</h4>
<p>Matching records this package against an existing resource. It does not merge records, copy permissions or authorize a connection. Missing values are unknown.</p>
<div class="candidate-comparison"><article v-for="candidate in candidates" :key="key(candidate.target)">
<h5>{{ label(candidate.kind) }}: {{ display(candidate.fields.find(v=>v.name==='name')) }}</h5>
<p>Exact resource: {{ candidate.target?.assetId }} · {{ candidate.target?.incarnation }}</p>
<a v-if="candidate.target" :href="`#/?asset=${encodeURIComponent(candidate.target.assetId)}&incarnation=${encodeURIComponent(candidate.target.incarnation)}`" target="_blank" rel="noopener">Inspect current resource and attachments</a>
<dl><template v-for="field in fields" :key="field"><dt>{{ field.replaceAll('_',' ') }}</dt><dd>Package: {{ finding.fields.some(v=>v.name===field)?display(finding.fields.find(v=>v.name===field)):'Unknown' }}</dd><dd>Declared: {{ candidate.fields.some(v=>v.name===field)?display(candidate.fields.find(v=>v.name===field)):'Unknown or unavailable' }}</dd></template></dl>
<button type="button" @click="$emit('confirm',candidate)">Confirm this existing resource</button>
</article></div></section></template>
<style scoped>
.candidate-comparison{display:grid;grid-template-columns:repeat(auto-fit,minmax(min(100%,18rem),1fr));gap:1rem}
article{min-width:0;border:1px solid var(--border,#ccd8dc);border-radius:.5rem;padding:.75rem;overflow-wrap:anywhere}
dt{font-weight:600;margin-top:.6rem}dd{margin:.2rem 0}
</style>
