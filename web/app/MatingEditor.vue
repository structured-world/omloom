<script setup lang="ts">
import {computed} from 'vue';
import {model} from './api';
const value=defineModel<model.PhysicalMating|undefined>();
const enabled=computed({get:()=>value.value!==undefined,set:(on:boolean)=>{value.value=on?model.PhysicalMating.create({profile:{issuer:'',key:'',basis:''}}):undefined;}});
const roles=[model.PhysicalMatingRole.SIDE_A,model.PhysicalMatingRole.SIDE_B,model.PhysicalMatingRole.SELF_MATING];
// A native required placeholder is empty; the wire enum stays unspecified until a side is chosen.
const side=computed<model.PhysicalMatingRole|''>({get:()=>value.value&&value.value.role!==model.PhysicalMatingRole.UNSPECIFIED?value.value.role:'',set:role=>{if(value.value)value.value.role=role===''?model.PhysicalMatingRole.UNSPECIFIED:role;}});
</script>
<template><fieldset class="mating">
<legend>Declared mating interface</legend>
<label><input v-model="enabled" type="checkbox"/>A sourced mating contract is known</label>
<template v-if="value?.profile">
<p>Use the same issuer, contract and revision for the two declared mates. This checks only that declaration, not signal compatibility or service health.</p>
<label>Contract issuer<input v-model="value.profile.issuer" required maxlength="160"/></label>
<label>Interface contract<input v-model="value.profile.key" required maxlength="160"/></label>
<label>Contract revision or source<input v-model="value.profile.basis" required maxlength="160"/></label>
<label>Mating side<select v-model="side" required><option value="" disabled>Choose a side</option><option v-for="role in roles" :key="role" :value="role">{{model.PhysicalMatingRole[role]?.toLowerCase().replaceAll('_',' ')}}</option></select></label>
</template>
</fieldset></template>
<style scoped>.mating{display:grid;gap:.5rem;min-width:0}label{display:block}input:not([type=checkbox]),select{display:block;box-sizing:border-box;width:100%;min-height:44px}p{margin:0}</style>
