<script setup lang="ts">
import { ref, onMounted } from 'vue';
defineProps<{ title: string }>();
const emit = defineEmits<{ close: [] }>();
const dialog = ref<HTMLDialogElement>();
onMounted(() => dialog.value?.showModal());
</script>
<template><dialog ref="dialog" @cancel.prevent="emit('close')"><header><h2>{{ title }}</h2><button type="button" aria-label="Close dialog" @click="emit('close')">×</button></header><slot /></dialog></template>
