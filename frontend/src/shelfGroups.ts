import type { ShelfEntry } from "./progress";
import type { SeriesMeta } from "./types";

/** A series stays a series even when only one volume is visible. */
export type ShelfItem =
  | { kind: "book"; entry: ShelfEntry }
  | { kind: "series"; series: SeriesMeta; volumes: ShelfEntry[] };

/** Collapse runs of series members into one card each (order preserved). */
export function groupBySeries(list: ShelfEntry[]): ShelfItem[] {
  const items: ShelfItem[] = [];
  const at = new Map<string, number>();
  for (const entry of list) {
    const series = entry.series;
    if (!series) {
      items.push({ kind: "book", entry });
      continue;
    }
    const seen = at.get(series.id);
    if (seen === undefined) {
      at.set(series.id, items.length);
      items.push({ kind: "series", series, volumes: [entry] });
    } else {
      const item = items[seen];
      if (item.kind === "series") item.volumes.push(entry);
    }
  }
  return items;
}
