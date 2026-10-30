<script setup lang="ts">
import { model } from './api';
defineProps<{ observations: model.HostIdentityObservation[] }>();
const kindLabel = (value: number) => model.HostIdentityKind[value]?.replace('HOST_IDENTITY_KIND_', '').toLowerCase().replaceAll('_', ' ');
const qualityLabel = (value: number) => model.HostIdentityQuality[value]?.replace('HOST_IDENTITY_QUALITY_', '').toLowerCase().replaceAll('_', ' ');
</script>
<template><section v-if="observations.length" aria-label="Identity evidence">
<h3>Identity evidence</h3>
<p>Source claims support comparison. Equal identifiers do not establish the same machine, authenticated identity or access rights.</p>
<dl v-for="(value, index) in observations" :key="index">
<template v-if="value.claim"><dt>{{ kindLabel(value.claim.kind) }}</dt><dd>{{ value.claim.namespace }} · {{ qualityLabel(value.claim.quality) }}</dd>
<dt>Identifier digest</dt><dd class="mono">{{ value.claim.digest }}</dd></template>
<dt>Source</dt><dd>{{ value.source }}</dd><dt>Observed</dt><dd>{{ new Date(Number(value.observedAt)).toISOString() }}</dd>
</dl></section></template>
