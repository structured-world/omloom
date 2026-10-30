<script setup lang="ts">
import { model, enumLabel } from './api';
defineProps<{ value: model.AddressDelta }>();
const format = (value: model.CurrentAddress) => `${value.address}${value.prefixLength === undefined ? '' : '/'+value.prefixLength} (${enumLabel(model.AddressStability, value.stability)} · ${enumLabel(model.AddressState, value.state)})`;
</script>
<template><section aria-label="Address changes" class="address-delta">
<h4>Interface address changes</h4><p>{{ value.source }}. {{ value.complete ? 'Complete refresh of this source only.' : 'Partial refresh; omitted addresses retain their previous observation dates.' }}</p>
<ul><li v-for="address in value.added" :key="'added/'+address.address">Add {{ format(address) }}</li>
<li v-for="address in value.removed" :key="'removed/'+address.address">Remove {{ format(address) }} from this source</li>
<li v-for="change in value.updated" :key="'updated/'+change.after?.address"><template v-if="change.before && change.after">Update {{ format(change.before) }} → {{ format(change.after) }}</template></li></ul>
<p v-if="!value.added.length && !value.removed.length && !value.updated.length">No address differences.</p>
<p>Other sources, intended assignments and service binds are unchanged. An address change does not create or merge an identity.</p>
</section></template>
<style scoped>.address-delta { overflow-wrap: anywhere; } li { margin-block: .4rem; }</style>
