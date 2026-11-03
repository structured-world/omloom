<script setup lang="ts">
import { ref } from 'vue';
import { call, model, key, identityName, AssetKind as K, type AssetRef } from './api';
const props = defineProps<{ item: model.Item; working: model.WorkingContext; writable: boolean }>();
const emit = defineEmits<{ open: [target: AssetRef]; edit: [subjects: model.Item[], membership?: model.SegmentMembership] }>();
const page = ref<model.SegmentMembershipPage>(); const busy = ref(false); const error = ref('');
async function load(cursor?: string) {
  if (!props.item.target) return;
  busy.value = true; error.value = '';
  try {
    const reply = await call({ oneofKind: 'segmentMemberships', segmentMemberships: model.SegmentMemberships.create({ targets: [props.item.target], cursor }) }, model.WorkingContext.create({ ...props.working, expected: undefined }));
    if (reply.oneofKind !== 'segmentMembershipPage') throw Error('Membership projection is unavailable.');
    page.value = reply.segmentMembershipPage;
  } catch(e) { error.value = String(e); } finally { busy.value = false; }
}
</script>
<template><section aria-label="L2 participation"><h3>L2 participation</h3><p>Explicit membership is independent of addressing and does not prove connectivity.</p><button :disabled="busy" @click="load()">{{ page ? 'Refresh memberships' : 'Show memberships' }}</button><button v-if="writable && item.kind === K.VIRTUAL_MACHINE" @click="emit('edit', [item])">Record shared segment</button>
<p v-if="error" role="alert">{{error}}</p><template v-if="page"><p v-if="!page.entries.length">No permitted assertions in this selection.</p><ol aria-label="Selected network projection"><li v-for="entry in page.entries" :key="entry.relation?.relationId"><span v-if="entry.subject && key(entry.subject.target) !== key(entry.source?.target)">{{identityName(entry.subject)}} / </span><button v-if="entry.source?.target" @click="emit('open', entry.source.target)">{{identityName(entry.source)}}</button><span v-if="entry.source?.kind === K.VIRTUAL_MACHINE"> · interface unknown</span><span aria-label="member of"> → </span><button v-if="entry.segment?.target" @click="emit('open', entry.segment.target)">{{identityName(entry.segment)}}</button><small> · assertion {{entry.relation?.relationId.slice(-8)}}</small><button v-if="writable && (entry.subject || entry.source?.kind === K.VIRTUAL_MACHINE)" @click="emit('edit', [entry.subject ?? entry.source!], entry)">Refine participation</button></li></ol><button v-if="page.next" :disabled="busy" @click="load(page.next)">Next membership page</button></template></section></template>
