<script setup lang="ts">
import { useId } from 'vue';
import { model } from './api';
const addresses = defineModel<model.InitialAddress[]>({ required: true });
const id = useId();
function vlan(index: number, event: Event) {
  const value = (event.target as HTMLInputElement).value;
  addresses.value[index]!.vlanTag = value === '' ? undefined : Number(value);
}
function addressValue(address: model.InitialAddress) {
  return address.prefixLength === undefined ? address.address : `${address.address}/${address.prefixLength}`;
}
function editAddress(index: number, event: Event) {
  addresses.value[index]!.address = (event.target as HTMLInputElement).value;
  addresses.value[index]!.prefixLength = undefined;
}
</script>
<template><section class="initial-addresses" aria-label="Initial IP addresses">
<div class="address-heading"><h3>IP addresses</h3><button type="button" :disabled="addresses.length >= 16" @click="addresses.push(model.InitialAddress.create())">Add IP address</button></div>
<div v-for="(address, index) in addresses" :key="index" class="address-row">
<label :for="id+'-ip-'+index">IP address {{ index+1 }}<input :id="id+'-ip-'+index" :value="addressValue(address)" placeholder="IPv4 or IPv6, optional /prefix" maxlength="128" autocomplete="off" @input="editAddress(index,$event)"></label>
<label :for="id+'-vlan-'+index">VLAN tag<input :id="id+'-vlan-'+index" :value="address.vlanTag" type="number" min="1" max="4094" step="1" placeholder="Unknown" :aria-label="'VLAN tag '+(index+1)" @input="vlan(index,$event)"></label>
<button type="button" :aria-label="'Remove IP address '+(index+1)" @click="addresses.splice(index,1)">×</button>
<small v-if="address.interfaceName" class="interface-name">Interface: {{ address.interfaceName }}</small>
</div><p class="hint">Optional local facts. An IP or VLAN tag does not assign an interface, network or VRF.</p>
</section></template>
<style scoped>
.initial-addresses { margin:16px 0; }
.address-heading { display:flex;align-items:center;justify-content:space-between;gap:8px; }
.address-heading h3 { margin:0; }
.address-row { display:grid;grid-template-columns:minmax(0,1fr) 100px 44px;gap:8px;align-items:end;margin-top:10px; }
.address-row label { min-width:0;margin:0; }
.address-row input { width:100%;min-width:0; }
.address-row > button { min-height:44px; }
.interface-name { grid-column:1/-1;color:#6e818b; }
.initial-addresses .hint { margin:8px 0 0; }
@media(max-width:700px) { .address-row { grid-template-columns:minmax(0,1fr) 90px 36px;gap:6px; } }
</style>
