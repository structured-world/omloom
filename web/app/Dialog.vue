<script setup lang="ts">
import { ref, onMounted, onBeforeUnmount, useId } from 'vue';
defineProps<{ title: string }>();
const emit = defineEmits<{ close: [] }>();
const dialog = ref<HTMLDialogElement>();
const titleId = useId();
onMounted(() => dialog.value?.showModal());
onBeforeUnmount(() => dialog.value?.close());
</script>
<template><dialog ref="dialog" :aria-labelledby="titleId" @cancel.prevent="emit('close')"><header><h2 :id="titleId">{{ title }}</h2><button type="button" aria-label="Close dialog" @click="emit('close')">×</button></header><slot /></dialog></template>
