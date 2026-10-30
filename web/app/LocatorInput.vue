<script setup lang="ts">
import { computed, onMounted, ref, watch } from 'vue';
import { parsePhoneNumberWithError } from 'libphonenumber-js/max';
import { model, enumLabel } from './api';
const props = defineProps<{ modelValue?: string; kind: model.LocatorKind; label: string; allowed?: model.LocatorKind[] }>();
const emit = defineEmits<{ 'update:modelValue':[value:string]; 'update:kind':[value:model.LocatorKind] }>();
const input = ref<HTMLInputElement>();
const kinds = computed(() => props.allowed ?? [model.LocatorKind.UNSPECIFIED, model.LocatorKind.EMAIL, model.LocatorKind.PHONE, model.LocatorKind.WEB, model.LocatorKind.DOMAIN]);
const phoneError = computed(() => {
  if (props.kind !== model.LocatorKind.PHONE || !props.modelValue) return '';
  try {
    const number = parsePhoneNumberWithError(props.modelValue, { extract:false });
    if (number.isValid() && number.number === props.modelValue) return '';
  } catch { /* Invalid or incomplete input stays editable. */ }
  return 'Enter a valid international E.164 number, for example +442079460018.';
});
function validity() { input.value?.setCustomValidity(phoneError.value); }
watch(phoneError, validity, { flush:'post' });
onMounted(validity);
</script>
<template><label>{{label}} type<select :aria-label="label+' type'" :value="kind" @change="emit('update:kind',Number(($event.target as HTMLSelectElement).value))"><option v-for="value in kinds" :key="value" :value="value">{{value === model.LocatorKind.UNSPECIFIED ? 'Unknown / other route' : enumLabel(model.LocatorKind,value)}}</option></select></label><label>{{label}}<input ref="input" :aria-label="label" :value="modelValue" :type="kind===model.LocatorKind.PHONE?'tel':'text'" maxlength="512" :aria-invalid="!!phoneError" @input="emit('update:modelValue',($event.target as HTMLInputElement).value)"></label><p v-if="phoneError" class="hint">{{phoneError}}</p><p v-else-if="kind!==model.LocatorKind.UNSPECIFIED" class="hint">The server validates this declared type. Syntax does not prove ownership or delivery.</p></template>
