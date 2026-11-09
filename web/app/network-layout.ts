export type Point = { x: number; y: number };
type Link = { from: string; to: string };

export const CARD_WIDTH = 168;
export const CARD_HEIGHT = 64;
const CELL_WIDTH = 224;
const CELL_HEIGHT = 112;

// Breadth-first placement groups related nodes, retaining every input link.
// Each node enters the queue once, even in cyclic and high-degree components.
export function networkOrder(keys: readonly string[], links: readonly Link[]): string[] {
  const neighbors = new Map(keys.map(key => [key, new Set<string>()]));
  for (const { from, to } of links) {
    neighbors.get(from)?.add(to);
    neighbors.get(to)?.add(from);
  }
  const order: string[] = [];
  const queued = new Set<string>();
  for (const key of keys) {
    if (queued.has(key)) continue;
    queued.add(key);
    const start = order.length;
    order.push(key);
    for (let index = start; index < order.length; index++) {
      for (const next of neighbors.get(order[index]!) ?? []) {
        if (queued.has(next) || !neighbors.has(next)) continue;
        queued.add(next);
        order.push(next);
      }
    }
  }
  return order;
}

export function networkPosition(slot: number, columns: number): Point {
  return { x: 32 + slot % columns * CELL_WIDTH, y: 32 + Math.floor(slot / columns) * CELL_HEIGHT };
}

export function networkPath(from: Point, to: Point, lane: number): string {
  if (from === to) {
    const x = from.x + CARD_WIDTH;
    const y = from.y + CARD_HEIGHT / 2;
    return `M ${x} ${y - 8} c ${24 + lane * 6} -26 ${24 + lane * 6} 26 0 16`;
  }
  const x = from.x + CARD_WIDTH / 2;
  const y = from.y + CARD_HEIGHT / 2;
  const dx = to.x - from.x;
  const dy = to.y - from.y;
  const distance = Math.hypot(dx, dy) || 1;
  const controlX = x + dx / 2 - dy / distance * lane * 16;
  const controlY = y + dy / 2 + dx / distance * lane * 16;
  return `M ${x} ${y} Q ${controlX} ${controlY} ${x + dx} ${y + dy}`;
}
