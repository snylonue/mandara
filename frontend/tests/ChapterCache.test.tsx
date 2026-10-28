import { expect, it, vi } from "vitest";
import { ChapterCache } from "../src/chapterCache";
import type { Chapter } from "../src/types";

const chapter = (idx: number, content = "正文"): Chapter => ({ idx, title: "章节", content });

it("evicts the least recently used chapter when the entry budget is reached", async () => {
  const cache = new ChapterCache({ maxEntries: 2 });
  const load = vi.fn(async idx => chapter(idx));
  await cache.load(0, load);
  await cache.load(1, load);
  expect(cache.get(0)?.idx).toBe(0);
  await cache.load(2, load);
  expect(cache.get(1)).toBeUndefined();
  expect(cache.get(0)?.idx).toBe(0);
  expect(cache.get(2)?.idx).toBe(2);
  await cache.load(0, load);
  expect(load).toHaveBeenCalledTimes(3);
});

it("bounds retained UTF-16 text and title bytes independently of the chapter count", async () => {
  const cache = new ChapterCache({ maxEntries: 10, maxTextBytes: 20 });
  const load = async (idx: number) => chapter(idx);
  await cache.load(0, load);
  await cache.load(1, load);
  await cache.load(2, load);
  expect(cache.get(0)).toBeUndefined();
  expect(cache.get(1)?.idx).toBe(1);
  expect(cache.get(2)?.idx).toBe(2);
  await cache.load(3, async idx => chapter(idx, "正文正文"));
  expect(cache.get(1)).toBeUndefined();
  expect(cache.get(2)?.idx).toBe(2);
  expect(cache.get(3)?.idx).toBe(3);
});

it("displays oversized chapters without retaining them or evicting useful neighbors", async () => {
  const cache = new ChapterCache({ maxTextBytes: 20 });
  await cache.load(0, async idx => chapter(idx));
  const oversized = chapter(1, "文".repeat(20));
  expect(await cache.load(1, async () => oversized)).toBe(oversized);
  expect(cache.get(1)).toBeUndefined();
  expect(cache.get(0)?.idx).toBe(0);
});

it("shares an in-flight request across repeated visits to a chapter", async () => {
  const cache = new ChapterCache();
  let finish!: (chapter: Chapter) => void;
  const load = vi.fn(() => new Promise<Chapter>(resolve => { finish = resolve; }));
  const first = cache.load(0, load);
  const second = cache.load(0, load);
  expect(second).toBe(first);
  await Promise.resolve();
  expect(load).toHaveBeenCalledTimes(1);
  finish(chapter(0));
  expect(await first).toBe(await second);
  expect(cache.get(0)?.idx).toBe(0);
});

it.each([false, true])("releases failed requests so explicit retries work after synchronous=%s failures", async synchronous => {
  const cache = new ChapterCache();
  const failure = new Error("章节尚未缓存");
  const load = vi.fn(() => {
    if (synchronous) throw failure;
    return Promise.reject(failure);
  });
  await expect(cache.load(0, load)).rejects.toBe(failure);
  expect(cache.get(0)).toBeUndefined();
  const retry = vi.fn(async idx => chapter(idx));
  expect(await cache.load(0, retry)).toEqual(chapter(0));
  expect(retry).toHaveBeenCalledTimes(1);
});

it("keeps separate readers and content sources isolated", async () => {
  const first = new ChapterCache();
  const second = new ChapterCache();
  await first.load(0, async idx => chapter(idx, "第一本书"));
  expect(second.get(0)).toBeUndefined();
  await second.load(0, async idx => chapter(idx, "第二本书"));
  expect(first.get(0)?.content).toBe("第一本书");
  expect(second.get(0)?.content).toBe("第二本书");
});
