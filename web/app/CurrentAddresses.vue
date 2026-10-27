<script setup lang="ts">
import { model, enumLabel } from './api';
defineProps<{value:model.InterfaceAddressView}>();
const stability=(value:model.AddressStability)=>enumLabel(model.AddressStability, value);
const state=(value:model.AddressState)=>enumLabel(model.AddressState, value);
</script>
<template><section v-if="value.coverage" aria-label="Current interface addresses">
  <h3>Current interface addresses</h3>
  <p class="hint">{{ value.coverage.source }}. Latest collection: {{ new Date(Number(value.coverage.observedAt)).toLocaleString() }}. {{ value.coverage.complete ? 'Complete selected source scope.' : 'Partial refresh; omitted facts preserve their earlier dates.' }}</p>
  <ul><template v-for="fact in value.observations" :key="fact.value?.address"><li v-if="fact.value">
    <span class="mono">{{ fact.value.address }}{{ fact.value.prefixLength === undefined ? '' : '/'+fact.value.prefixLength }}</span>
    <small>{{ stability(fact.value.stability) }} · {{ state(fact.value.state) }} ·
      <template v-if="fact.observedAt !== undefined">Observed <time :datetime="new Date(Number(fact.observedAt)).toISOString()">{{ new Date(Number(fact.observedAt)).toLocaleString() }}</time></template>
      <template v-else>Observation time unavailable in the migrated partial collection.</template>
    </small>
  </li></template></ul>
  <p v-if="!value.observations.length" class="hint">No retained addresses in the supplied source scope. Temporary addresses are not retained.</p>
</section></template>
<style scoped>li{overflow-wrap:anywhere;padding:6px 0}small{display:block;color:#58717a}</style>
