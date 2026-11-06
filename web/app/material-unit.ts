// At most 64 sparse allocations are inspected, independently of requested quantity.
export function firstFreeUnit(material: import('./api').model.PhysicalMaterial): number | undefined {
  const occupied = new Set(material.allocations.map(value => value.unit));
  for (let unit = 0; unit <= occupied.size && unit < material.quantity; unit++) {
    if (!occupied.has(unit)) return unit;
  }
  return undefined;
}
