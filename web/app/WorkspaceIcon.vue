<script setup lang="ts">
import { model } from './api';
defineProps<{ view: model.View }>();
const paths: Record<model.View, string> = {
  [model.View.OVERVIEW]: 'M3 3h7v7H3z M14 3h7v7h-7z M3 14h7v7H3z M14 14h7v7h-7z',
  [model.View.NETWORK]: 'M4 7h16 M16 3l4 4-4 4 M20 17H4 M8 13l-4 4 4 4',
  [model.View.HARDWARE]: 'M4 3h16v18H4z M4 9h16 M4 15h16 M8 6h1 M8 12h1 M8 18h1',
  [model.View.VIRTUALIZATION]: 'M3 4h18v16H3z M9 4v16 M15 4v16',
  [model.View.SERVICES]: 'M12 3l9 5-9 5-9-5z M3 12l9 5 9-5 M3 16l9 5 9-5',
};
</script>
<template><svg class="workspace-icon" viewBox="0 0 24 24" aria-hidden="true" focusable="false"><path :d="paths[view]" /></svg></template>
