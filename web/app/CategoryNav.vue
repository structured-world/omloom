<script setup lang="ts">
import { computed } from 'vue';
import { model, AssetKind as K } from './api';
const props=defineProps<{view:model.View;kinds:number[]}>();
const emit=defineEmits<{select:[kinds:number[]]}>();
const categories:Record<model.View,{name:string;kinds:number[]}[]>={
  [model.View.OVERVIEW]:[],
  [model.View.HARDWARE]:[
    {name:'Devices',kinds:[K.DEVICE]},{name:'Sites & locations',kinds:[K.SITE,K.LOCATION]},
    {name:'Racks',kinds:[K.RACK]},{name:'Physical ports',kinds:[K.PORT]},
    {name:'Cables',kinds:[K.CABLE]},{name:'Power feeds',kinds:[K.POWER_FEED]},
  ],
  [model.View.NETWORK]:[
    {name:'Interfaces & LAGs',kinds:[K.INTERFACE,K.LAG]},
    {name:'Address spaces',kinds:[K.ADDRESS_SPACE]},{name:'Prefixes',kinds:[K.PREFIX]},
    {name:'Allocated IPs',kinds:[K.IP_ADDRESS]},{name:'Known addresses',kinds:[K.ADDRESS_REFERENCE]},
    {name:'VRFs',kinds:[K.VRF]},{name:'VLANs & L2 domains',kinds:[K.VLAN,K.L2_DOMAIN]},
    {name:'Shared IP groups',kinds:[K.ADDRESS_GROUP]},{name:'Ports & cables',kinds:[K.PORT,K.CABLE]},
  ],
  [model.View.VIRTUALIZATION]:[
    {name:'Virtual machines',kinds:[K.VIRTUAL_MACHINE]},{name:'Hypervisors',kinds:[K.SOFTWARE_INSTALLATION]},
    {name:'Clusters',kinds:[K.CLUSTER]},
  ],
  [model.View.SERVICES]:[
    {name:'Services',kinds:[K.SERVICE]},{name:'OS & software installations',kinds:[K.SOFTWARE_INSTALLATION]},
    {name:'Products',kinds:[K.SOFTWARE_PRODUCT]},{name:'Releases',kinds:[K.SOFTWARE_RELEASE]},
    {name:'Workloads',kinds:[K.WORKLOAD]},{name:'Containers',kinds:[K.CONTAINER]},
    {name:'Storage',kinds:[K.STORAGE]},
    {name:'Domains',kinds:[K.DOMAIN]},{name:'Provider accounts',kinds:[K.PROVIDER_ACCOUNT]},
  ],
};
const items=computed(()=>categories[props.view]);
function selected(kinds:number[]) { return kinds.length===props.kinds.length && kinds.every(k=>props.kinds.includes(k)); }
</script>
<template><nav v-if="items.length" class="category-nav" aria-label="Resource categories"><button :aria-pressed="!kinds.length" @click="emit('select',[])">All</button><button v-for="category in items" :key="category.name" :aria-pressed="selected(category.kinds)" @click="emit('select',category.kinds)">{{ category.name }}</button></nav></template>
