import type { Chapter } from "./types";

/** Bound retained chapter text, independently of the currently displayed DOM. */
export class ChapterCache {
  private readonly chapters = new Map<number, { chapter: Chapter; bytes: number }>();
  private readonly pending = new Map<number, Promise<Chapter>>();
  private textBytes = 0;
  private readonly maxEntries: number;
  private readonly maxTextBytes: number;

  constructor({ maxEntries = 8, maxTextBytes = 8 * 1024 * 1024 } = {}) {
    this.maxEntries = maxEntries;
    this.maxTextBytes = maxTextBytes;
  }

  get(idx: number): Chapter | undefined {
    const entry = this.chapters.get(idx);
    if (!entry) return;
    this.chapters.delete(idx);
    this.chapters.set(idx, entry);
    return entry.chapter;
  }

  load(idx: number, loader: (idx: number) => Promise<Chapter>): Promise<Chapter> {
    const cached = this.get(idx);
    if (cached) return Promise.resolve(cached);
    const pending = this.pending.get(idx);
    if (pending) return pending;
    // Defer invocation so synchronous loader failures also release the pending slot.
    const request = Promise.resolve().then(() => loader(idx)).then(chapter => {
      // UTF-16 text estimate, not an accounting of total browser heap usage.
      const bytes = (chapter.content.length + chapter.title.length) * 2;
      if (this.maxEntries > 0 && bytes <= this.maxTextBytes) {
        while (this.chapters.size >= this.maxEntries || this.textBytes + bytes > this.maxTextBytes) {
          const oldest = this.chapters.keys().next().value!;
          this.textBytes -= this.chapters.get(oldest)!.bytes;
          this.chapters.delete(oldest);
        }
        this.chapters.set(idx, { chapter, bytes });
        this.textBytes += bytes;
      }
      return chapter;
    }).finally(() => { this.pending.delete(idx); });
    this.pending.set(idx, request);
    return request;
  }
}
