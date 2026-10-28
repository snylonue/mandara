import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { Reader } from "../src/components/Reader";
import { createReaderProgress } from "../src/readerProgress";

const { epubRender } = vi.hoisted(() => ({ epubRender: vi.fn() }));
vi.mock("../src/components/EpubDocument", () => ({
  EpubDocument: (props: { onFraction: (fraction: number) => void }) => {
    epubRender(props);
    return <p>模拟 EPUB 正文</p>;
  },
}));

beforeEach(() => {
  localStorage.clear();
  HTMLElement.prototype.scrollTo = vi.fn();
  HTMLElement.prototype.scrollIntoView = vi.fn();
  epubRender.mockClear();
});
afterEach(() => {
  cleanup();
  vi.useRealTimers();
  vi.restoreAllMocks();
  delete (HTMLElement.prototype as Partial<HTMLElement>).scrollTo;
  delete (HTMLElement.prototype as Partial<HTMLElement>).scrollIntoView;
});

it("coalesces visual updates and cancels stale snapshots on reset or disposal", () => {
  vi.useFakeTimers();
  const store = createReaderProgress(0);
  const listener = vi.fn();
  const unsubscribe = store.subscribe(listener);
  for (let i = 1; i <= 100; i++) store.update(i / 100);
  expect(store.getSnapshot()).toBe(0);
  expect(listener).not.toHaveBeenCalled();
  vi.advanceTimersByTime(100);
  expect(store.getSnapshot()).toBe(1);
  expect(listener).toHaveBeenCalledTimes(1);
  store.update(0.8);
  store.reset(0);
  vi.advanceTimersByTime(100);
  expect(store.getSnapshot()).toBe(0);
  expect(listener).toHaveBeenCalledTimes(2);
  store.update(0.5);
  store.cancel();
  vi.advanceTimersByTime(100);
  expect(store.getSnapshot()).toBe(0);
  unsubscribe();
  store.reset(1);
  expect(listener).toHaveBeenCalledTimes(2);
});

async function reader(epub: boolean, onProgress = vi.fn(), readOnly = false) {
  const chapters = [{ idx: 0, title: "第一章", linear: true }, { idx: 1, title: "第二章", linear: true }];
  const view = render(<Reader title="测试书籍" chapters={chapters}
    toc={chapters.map(chapter => ({ ...chapter, children: [], frag: null }))}
    loadChapter={async idx => ({ idx, title: chapters[idx].title, content: epub ? "<!doctype html><html><body>正文</body></html>" : "<p>纯文本正文</p>" })}
    onProgress={onProgress} readOnly={readOnly} />);
  await screen.findByText(epub ? "模拟 EPUB 正文" : "纯文本正文");
  const viewport = view.container.querySelector<HTMLElement>(".reader-viewport")!;
  Object.defineProperties(viewport, { scrollHeight: { value: 2000 }, clientHeight: { value: 1000 } });
  function scroll(fraction: number) {
    if (epub) act(() => epubRender.mock.lastCall![0].onFraction(fraction));
    else { viewport.scrollTop = fraction * 1000; fireEvent.scroll(viewport); }
  }
  return { ...view, scroll };
}

it.each([false, true])("throttles visual progress but saves the exact latest fraction for EPUB=%s", async epub => {
  const onProgress = vi.fn();
  const view = await reader(epub, onProgress);
  fireEvent.click(screen.getByRole("button", { name: "目录" }));
  const renderCount = epubRender.mock.calls.length;
  vi.useFakeTimers();
  for (let i = 1; i <= 40; i++) view.scroll(i / 100);
  expect(view.container.querySelector(".reader-topbar-progress")!.textContent).toBe("0%");
  act(() => vi.advanceTimersByTime(100));
  expect(view.container.querySelector(".reader-topbar-progress")!.textContent).toBe("20%");
  expect(view.container.querySelector<HTMLElement>(".progress-fill")!.style.transform).toBe("scaleX(0.2)");
  expect(epubRender).toHaveBeenCalledTimes(renderCount);
  expect(onProgress).not.toHaveBeenCalled();
  view.scroll(0.4827);
  act(() => vi.advanceTimersByTime(599));
  expect(onProgress).not.toHaveBeenCalled();
  act(() => vi.advanceTimersByTime(1));
  expect(onProgress.mock.calls).toEqual([[0.4827, 0]]);
  view.unmount();
  expect(onProgress).toHaveBeenCalledTimes(1);
});

it.each([false, true])("flushes pending positions once, including zero, on EPUB=%s unmount", async epub => {
  const onProgress = vi.fn();
  const view = await reader(epub, onProgress);
  vi.useFakeTimers();
  view.scroll(0.63);
  view.scroll(0);
  view.unmount();
  expect(onProgress.mock.calls).toEqual([[0, 0]]);
  act(() => vi.runAllTimers());
  expect(onProgress).toHaveBeenCalledTimes(1);
});

it("flushes the previous chapter before resetting navigation progress", async () => {
  const onProgress = vi.fn();
  const view = await reader(false, onProgress);
  view.scroll(0.7421);
  fireEvent.click(screen.getByRole("button", { name: /下一章/ }));
  await screen.findByText("纯文本正文");
  expect(onProgress.mock.calls).toEqual([[0.7421, 0], [0, 1]]);
  expect(view.container.querySelector(".reader-topbar-progress")!.textContent).toBe("50%");
  view.unmount();
  expect(onProgress).toHaveBeenCalledTimes(2);
});

it("keeps visual updates pending when persistence callbacks change", async () => {
  const previous = vi.fn();
  const next = vi.fn();
  const view = await reader(false, previous);
  vi.useFakeTimers();
  view.scroll(0.6);
  view.rerender(<Reader title="测试书籍"
    chapters={[{ idx: 0, title: "第一章", linear: true }, { idx: 1, title: "第二章", linear: true }]}
    loadChapter={async idx => ({ idx, title: "第一章", content: "<p>纯文本正文</p>" })} onProgress={next} />);
  expect(previous.mock.calls).toEqual([[0.6, 0]]);
  act(() => vi.advanceTimersByTime(100));
  expect(view.container.querySelector(".reader-topbar-progress")!.textContent).toBe("30%");
  view.scroll(0.72);
  act(() => vi.advanceTimersByTime(600));
  expect(next.mock.calls).toEqual([[0.72, 0]]);
  view.unmount();
  expect(previous).toHaveBeenCalledTimes(1);
  expect(next).toHaveBeenCalledTimes(1);
});

it("does not let a pending scroll overwrite a same-chapter restart", async () => {
  const onProgress = vi.fn();
  const view = await reader(false, onProgress);
  fireEvent.click(screen.getByRole("button", { name: "目录" }));
  vi.useFakeTimers();
  view.scroll(0.8);
  fireEvent.click(screen.getByRole("button", { name: "第一章" }));
  act(() => vi.advanceTimersByTime(1000));
  expect(onProgress.mock.calls).toEqual([[0, 0]]);
  expect(view.container.querySelector(".reader-topbar-progress")!.textContent).toBe("0%");
});

it.each([false, true])("updates read-only EPUB=%s progress without persistence", async epub => {
  const onProgress = vi.fn();
  const view = await reader(epub, onProgress, true);
  vi.useFakeTimers();
  view.scroll(0.8);
  act(() => vi.advanceTimersByTime(1000));
  expect(view.container.querySelector(".reader-topbar-progress")).toBeNull();
  expect(view.container.querySelector<HTMLElement>(".progress-fill")!.style.transform).toBe("scaleX(0.4)");
  view.unmount();
  expect(onProgress).not.toHaveBeenCalled();
});
