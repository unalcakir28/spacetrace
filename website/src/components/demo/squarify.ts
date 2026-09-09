/**
 * Squarified treemap layout — Bruls, Huizing and van Wijk, 1999.
 *
 * The same algorithm the desktop application uses, in the same order: children
 * biggest first, laid into rows along the shorter side, closing a row when
 * adding the next item would make its worst aspect ratio worse. A naive
 * slice-and-dice layout produces slivers you cannot click, which for a map you
 * are meant to navigate is the whole ball game.
 */

export interface Rect {
  x: number;
  y: number;
  w: number;
  h: number;
}

export interface Placed<T> {
  item: T;
  rect: Rect;
}

/** The worst aspect ratio in a row of areas laid along `side`. */
function worst(areas: number[], side: number, sum: number): number {
  if (sum <= 0 || side <= 0) return Infinity;
  const rowThickness = sum / side;
  let max = 0;
  for (const area of areas) {
    const length = area / rowThickness;
    const ratio = Math.max(length / rowThickness, rowThickness / length);
    if (ratio > max) max = ratio;
  }
  return max;
}

/**
 * @param items  values in any order; they are sorted descending here
 * @param bounds the rectangle to fill
 * @returns one rectangle per item, in draw order
 */
export function squarify<T>(
  items: { item: T; value: number }[],
  bounds: Rect,
): Placed<T>[] {
  const positive = items.filter((entry) => entry.value > 0);
  const total = positive.reduce((sum, entry) => sum + entry.value, 0);
  if (total <= 0 || bounds.w <= 0 || bounds.h <= 0) return [];

  // Work in area units so the ratio maths is in the same space as the
  // rectangle, then no conversion is needed when a row is emitted.
  const scale = (bounds.w * bounds.h) / total;
  const queue = [...positive]
    .sort((a, b) => b.value - a.value)
    .map((entry) => ({ item: entry.item, area: entry.value * scale }));

  const out: Placed<T>[] = [];
  let free: Rect = { ...bounds };
  let row: { item: T; area: number }[] = [];
  let rowArea = 0;

  const side = () => Math.min(free.w, free.h);

  const emitRow = () => {
    if (!row.length) return;
    const thickness = rowArea / side();
    const horizontal = free.w >= free.h;

    let offset = 0;
    for (const entry of row) {
      const length = entry.area / thickness;
      out.push({
        item: entry.item,
        rect: horizontal
          ? { x: free.x, y: free.y + offset, w: thickness, h: length }
          : { x: free.x + offset, y: free.y, w: length, h: thickness },
      });
      offset += length;
    }

    // Shrink the free rectangle by the strip that was just filled.
    free = horizontal
      ? { x: free.x + thickness, y: free.y, w: free.w - thickness, h: free.h }
      : { x: free.x, y: free.y + thickness, w: free.w, h: free.h - thickness };

    row = [];
    rowArea = 0;
  };

  for (const entry of queue) {
    const areas = row.map((r) => r.area);
    const current = worst(areas, side(), rowArea);
    const next = worst([...areas, entry.area], side(), rowArea + entry.area);

    // Adding this item makes the row's worst tile squarer, so keep going.
    if (row.length === 0 || next <= current) {
      row.push(entry);
      rowArea += entry.area;
      continue;
    }

    emitRow();
    row.push(entry);
    rowArea += entry.area;
  }
  emitRow();

  return out;
}
