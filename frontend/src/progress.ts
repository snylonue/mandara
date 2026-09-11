// Reading progress for the shelf: the caller's sessions (`GET /api/sessions`)
// resolved against the chapter counts the book lists already carry.
//
// A file's progress is `(chapter_idx + in-chapter fraction) / chapter_count`
// — the same notion the reader saves. Only `fraction` is written by the web
// reader, so an `offset`-based session (non-web readers) counts as the start
// of its chapter.
import type { BookListEntry, ReadingSession, SessionList } from "./types";

/** Fraction of a file from which it counts as read. */
export const FINISHED_AT = 0.98;

export interface FileProgress {
  fileId: string;
  session: ReadingSession;
  /** 0..1 across the file's chapters. */
  percent: number;
}

/** Anything with a file id and a chapter count (FileMeta). */
type FileLike = { id: string; chapter_count: number };

export function sessionPercent(session: ReadingSession, chapterCount: number): number {
  if (chapterCount <= 0) return 0;
  const { chapter_idx, in_chapter } = session.position;
  const frac = "fraction" in in_chapter ? in_chapter.fraction : 0;
  return Math.min(1, Math.max(0, (chapter_idx + frac) / chapterCount));
}

/**
 * Best (most advanced) session per file. `chapterCountOf` maps a file id to
 * its chapter count; files the caller cannot see any more return 0 and are
 * therefore dropped unless they carry an explicit chapter index.
 */
export function progressByFile(
  sessions: SessionList,
  chapterCountOf: (fileId: string) => number,
): Map<string, FileProgress> {
  const byFile = new Map<string, FileProgress>();
  for (const session of sessions) {
    const percent = sessionPercent(session, chapterCountOf(session.file_id));
    const prev = byFile.get(session.file_id);
    if (!prev || percent > prev.percent) {
      byFile.set(session.file_id, { fileId: session.file_id, session, percent });
    }
  }
  return byFile;
}

/** An entry plus the caller's progress in it. */
export type WithProgress<T> = T & { progress: FileProgress | null };

/** A shelf entry: a book list entry plus the caller's progress in it. */
export type ShelfEntry = WithProgress<BookListEntry>;

/**
 * Attach per-book progress to a book list (keeps the list order). Generic
 * over the entry shape so `BookDetail` rows work too.
 */
export function attachProgress<T extends { files: FileLike[] }>(
  entries: T[],
  byFile: Map<string, FileProgress>,
): WithProgress<T>[] {
  return entries.map((entry) => ({ ...entry, progress: bookProgress(entry.files, byFile) }));
}

/** Most advanced progress among the files of one book (null = untouched). */
export function bookProgress(
  files: FileLike[],
  byFile: Map<string, FileProgress>,
): FileProgress | null {
  let best: FileProgress | null = null;
  for (const f of files) {
    const p = byFile.get(f.id);
    if (p && (!best || p.percent > best.percent)) best = p;
  }
  return best;
}

export const isFinished = (p: FileProgress | null): boolean =>
  !!p && p.percent >= FINISHED_AT;

export const hasStarted = (p: FileProgress | null): boolean => !!p && p.percent > 0;

/** Whole-percent integer for display (0..100). */
export const percentLabel = (p: FileProgress | null): number =>
  p ? Math.round(p.percent * 100) : 0;
