<script setup lang="ts">
import { ref } from 'vue';
import Dialog from './Dialog.vue';
const props = defineProps<{ title: string; options: { id: string; name: string }[]; selected: string[]; allowEmpty: boolean }>();
const emit = defineEmits<{ close: []; apply: [selected: string[]] }>();
const draft = ref([...props.selected]);
</script>
<template>
  <Dialog :title="title" @close="emit('close')">
    <form @submit.prevent="emit('apply', draft)">
      <p class="hint">{{ allowEmpty ? 'No selection includes all resource kinds.' : 'Choose at least one column. Resource identity remains available.' }}</p>
      <fieldset class="choice-list"><legend>{{ title }}</legend>
        <label v-for="option in options" :key="option.id" class="check"><input v-model="draft" type="checkbox" :value="option.id">{{ option.name }}</label>
      </fieldset>
      <footer><button v-if="allowEmpty" type="button" @click="draft = []">Clear choices</button><button type="button" @click="emit('close')">Cancel</button><button class="primary" :disabled="!allowEmpty && !draft.length">Apply selection</button></footer>
    </form>
  </Dialog>
</template>
