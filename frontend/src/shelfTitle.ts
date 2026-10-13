import type { BookListEntry } from "./types";

/** Qualify ordinal-only volume labels without rewriting meaningful source titles. */
export function shelfBookTitle(entry: Pick<BookListEntry, "book" | "series">): string {
  const { title } = entry.book;
  const seriesTitle = entry.series?.title;
  const ordinalOnly = /^第\s*[0-9０-９〇零一二三四五六七八九十百千万两]+\s*[卷巻册]$/u.test(title.trim());
  if (!seriesTitle || !ordinalOnly || title === seriesTitle) return title;
  return seriesTitle + " · " + title;
}
