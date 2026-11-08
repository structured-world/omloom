<script setup lang="ts">
import { computed, ref, watch } from 'vue';
import { key, label, model, RelationKind, type AssetRef } from './api';
import { CARD_HEIGHT, CARD_WIDTH, networkOrder, networkPath, networkPosition, type Point } from './network-layout';

const props = defineProps<{
  page: model.Page;
  pins: model.NetworkPin[];
  selected?: AssetRef;
  name: (item: model.Item) => string;
}>();
const emit = defineEmits<{ open: [target: AssetRef]; pin: [pin: model.NetworkPin]; clear: [] }>();
const arrange = ref(false);
const linksOpen = ref(false);
const zoom = ref(1);
const scroll = ref<HTMLElement>();
const stage = ref<HTMLElement>();
const position = ref(new Map<string, number>());
const links = computed(() => (props.page.topology?.edges ?? []).map(edge => ({
  edge, from: key(edge.from), to: key(edge.to),
})));
const order = computed(() => networkOrder(props.page.items.map(item => key(item.target)), links.value));
function place(reset = false) {
  const next = new Map<string, number>();
  if (!reset) for (const id of order.value) {
    const slot = position.value.get(id);
    if (slot !== undefined && slot < order.value.length) next.set(id, slot);
  }
  const used = new Set(next.values());
  let slot = 0;
  for (const id of order.value) {
    if (next.has(id)) continue;
    while (used.has(slot)) slot++;
    next.set(id, slot++);
  }
  position.value = next;
}
watch(order, () => place(), { immediate: true });
const pins = computed(() => new Map(props.pins.map(pin => [key(pin.target), pin])));
const prefixTitles = computed(() => new Map(props.page.contexts.map(context => [key(context.target), context.prefixCidr])));
const labels = computed(() => {
  const names = props.page.items.map(item => props.name(item) || 'Unnamed resource');
  const counts = new Map<string, number>();
  for (const name of names) counts.set(name, (counts.get(name) ?? 0) + 1);
  return new Map(props.page.items.map((item, index) => {
    const name = names[index]!;
    const caption = (counts.get(name) ?? 0) > 1 && item.target ? `${name} · ${item.target.assetId.slice(-8)}` : name;
    return [key(item.target), caption];
  }));
});
const nodes = computed(() => props.page.items.map(item => {
  const id = key(item.target);
  const base = networkPosition(position.value.get(id) ?? 0, 4);
  const pin = pins.value.get(id);
  const point = {
    x: pin?.x ?? base.x,
    y: pin?.y ?? base.y,
  };
  return { item, id, point };
}));
const paths = computed(() => {
  const points = new Map(nodes.value.map(node => [node.id, node.point]));
  const lanes = new Map<string, number>();
  return links.value.flatMap(({ edge, from, to }) => {
    const a = points.get(from);
    const b = points.get(to);
    if (!a || !b) return [];
    const pair = from < to ? `${from}/${to}` : `${to}/${from}`;
    const lane = lanes.get(pair) ?? 0;
    lanes.set(pair, lane + 1);
    const offset = lane % 2 ? -(lane + 1) / 2 : lane / 2;
    return [{ edge, path: networkPath(a, b, from <= to ? offset : -offset) }];
  });
});
// Move the canvas origin into view without changing the stored coordinates.
const offset = computed(() => ({
  x: 8 - Math.min(8, ...nodes.value.map(n => n.point.x)),
  y: 8 - Math.min(8, ...nodes.value.map(n => n.point.y)),
}));
const width = computed(() => Math.max(260, ...nodes.value.map(n => n.point.x + offset.value.x + CARD_WIDTH + 40)));
const height = computed(() => Math.max(152, ...nodes.value.map(n => n.point.y + offset.value.y + CARD_HEIGHT + 40)));
const minimumZoom = computed(() => Math.min(.35, (scroll.value?.clientWidth || width.value) / width.value));
function fit() {
  if (!scroll.value?.clientWidth) return;
  zoom.value = Math.min(1, scroll.value.clientWidth / width.value);
  scroll.value.scrollTo(0, 0);
}
function move(node: typeof nodes.value[number], x: number, y: number) {
  if (!node.item.target) return;
  emit('pin', {
    target: node.item.target,
    x: Math.round(Math.max(-4096, Math.min(4096, x))),
    y: Math.round(Math.max(-4096, Math.min(4096, y))),
  });
}
let drag: { id: string; dx: number; dy: number; pointer: number } | undefined;
function start(event: PointerEvent, node: typeof nodes.value[number]) {
  if (!arrange.value || event.button !== 0 || !stage.value) return;
  const rect = stage.value.getBoundingClientRect();
  drag = { id: node.id, dx: (event.clientX - rect.left) / zoom.value - offset.value.x - node.point.x, dy: (event.clientY - rect.top) / zoom.value - offset.value.y - node.point.y, pointer: event.pointerId };
  (event.currentTarget as HTMLElement).setPointerCapture(event.pointerId);
}
function update(event: PointerEvent, node: typeof nodes.value[number]) {
  if (drag?.id !== node.id || drag.pointer !== event.pointerId || !stage.value) return;
  const rect = stage.value.getBoundingClientRect();
  move(node, (event.clientX - rect.left) / zoom.value - offset.value.x - drag.dx, (event.clientY - rect.top) / zoom.value - offset.value.y - drag.dy);
}
function moveWithKey(event: KeyboardEvent, node: typeof nodes.value[number]) {
  if (!arrange.value) return;
  const delta = { ArrowLeft: [-16, 0], ArrowRight: [16, 0], ArrowUp: [0, -16], ArrowDown: [0, 16] }[event.key];
  if (!delta) return;
  event.preventDefault();
  move(node, node.point.x + delta[0]!, node.point.y + delta[1]!);
}
function inspect(target?: AssetRef) {
  if (!arrange.value && target) emit('open', target);
}
</script>

<template>
  <section class="network-topology" aria-label="Current network topology">
    <div class="network-topology-tools">
      <div><h2>Known network</h2><p>Recorded relationships on this page. Missing links do not establish isolation or reachability.</p></div>
      <div v-if="nodes.length" class="actions">
        <button @click="fit">Fit</button>
        <button aria-label="Zoom out" :disabled="zoom <= minimumZoom" @click="zoom = Math.max(minimumZoom, zoom / 1.2)">−</button>
        <button aria-label="Zoom in" :disabled="zoom >= 1.8" @click="zoom = Math.min(1.8, zoom * 1.2)">+</button>
        <button :aria-pressed="arrange" @click="arrange = !arrange">{{ arrange ? 'Done arranging' : 'Arrange' }}</button>
        <button v-if="arrange" @click="place(true); emit('clear')">Reset positions</button>
      </div>
    </div>
    <p v-if="arrange" class="hint">Drag a card or use its arrow keys. Save view to keep these positions.</p>
    <p v-if="page.topology?.moreEdges" class="hint">More recorded links are available in resource details.</p>
    <div v-if="nodes.length" ref="scroll" class="network-topology-scroll" tabindex="0" aria-label="Scrollable network diagram">
      <div class="network-topology-frame" :style="{ width: width * zoom + 'px', height: height * zoom + 'px' }">
        <div ref="stage" class="network-topology-stage" :class="{ arranging: arrange }"
          :style="{ width: width + 'px', height: height + 'px', transform: `scale(${zoom})` }">
          <svg aria-hidden="true" :width="width" :height="height" :viewBox="`${-offset.x} ${-offset.y} ${width} ${height}`">
            <path v-for="link in paths" :key="link.edge.relationId" :d="link.path"
              :class="{ membership: [RelationKind.ON, RelationKind.MEMBER_OF].includes(link.edge.kind) }" />
          </svg>
          <button v-for="node in nodes" :key="node.id" class="network-topology-node"
            :title="prefixTitles.get(node.id)"
            :style="{ left: node.point.x + 'px', top: node.point.y + 'px', translate: `${offset.x}px ${offset.y}px`, width: CARD_WIDTH + 'px', height: CARD_HEIGHT + 'px' }"
            :aria-current="node.id === key(selected) ? 'true' : undefined" @click="inspect(node.item.target)"
            @pointerdown="start($event, node)" @pointermove="update($event, node)" @lostpointercapture="drag = undefined"
            @keydown="moveWithKey($event, node)">
            <small aria-hidden="true">{{ label(node.item.kind) }}</small>
            <strong>{{ labels.get(node.id) }}</strong>
          </button>
        </div>
      </div>
    </div>
    <details v-if="links.length" class="network-topology-links" @toggle="linksOpen = ($event.target as HTMLDetailsElement).open">
      <summary>Recorded links on this page</summary>
      <ul v-if="linksOpen">
        <li v-for="{ edge } in links" :key="edge.relationId">
          {{ labels.get(key(edge.from)) }}
          · {{ RelationKind[edge.kind]?.replaceAll('_', ' ').toLowerCase() }} ·
          {{ labels.get(key(edge.to)) }}
        </li>
      </ul>
    </details>
  </section>
</template>
