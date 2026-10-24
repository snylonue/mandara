import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { Reader } from "../src/components/Reader";
import * as epubScreenshot from "../src/epubScreenshot";
import { MAX_SCREENSHOT_TEXT, renderReaderScreenshot, selectedChapterText, selectedTextContent, wrapScreenshotText } from "../src/readerScreenshot";
import { SCREENSHOT_TIMEOUT } from "../src/screenshotTask";

const style = { background: "#fff", foreground: "#111", muted: "#666", accent: "#369", fontFamily: "serif" };
const drawn = vi.fn();
beforeEach(() => {
  localStorage.clear();
  HTMLElement.prototype.scrollTo = vi.fn();
  vi.stubGlobal("ResizeObserver", class { observe() {} disconnect() {} });
  vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockReturnValue({ measureText: (s: string) => ({ width: s.length * 28 }), fillRect: vi.fn(), fillText: drawn, drawImage: vi.fn(), scale: vi.fn() } as unknown as CanvasRenderingContext2D);
  vi.spyOn(HTMLCanvasElement.prototype, "toBlob").mockImplementation(callback => callback(new Blob(["png"], { type: "image/png" })));
  Object.defineProperty(URL, "createObjectURL", { configurable: true, value: vi.fn(() => "blob:preview") });
  Object.defineProperty(URL, "revokeObjectURL", { configurable: true, value: vi.fn() });
  drawn.mockClear();
});
afterEach(() => { document.getSelection()?.removeAllRanges(); vi.restoreAllMocks(); vi.unstubAllGlobals(); vi.useRealTimers(); });

function select(root: Node, start = 0, end = root.textContent!.length) {
  const doc = root.ownerDocument!;
  const range = doc.createRange();
  range.setStart(root, start);
  range.setEnd(root, end);
  doc.getSelection()!.removeAllRanges();
  doc.getSelection()!.addRange(range);
  fireEvent(doc, new Event("selectionchange"));
}

it("wraps CJK, blank lines and emoji without splitting graphemes or losing text", () => {
  const lines = wrapScreenshotText("甲乙丙\n\n👩‍💻丁", 2, s => Array.from(new Intl.Segmenter(undefined, { granularity: "grapheme" }).segment(s)).length);
  expect(lines).toEqual(["甲乙", "丙", "", "👩‍💻丁"]);
  const link = "https://example.com/" + "a".repeat(100);
  expect(wrapScreenshotText(link, 20, s => s.length).join("")).toBe(link);
});

it("only extracts a nonempty range wholly within chapter content", () => {
  const root = document.createElement("div");
  root.innerHTML = "<p>第一段</p><p>第二段</p>";
  const outside = document.createTextNode("工具栏");
  document.body.append(root, outside);
  try {
    select(root.querySelector("p")!.firstChild!, 1, 3);
    expect(selectedChapterText(root, document.getSelection())).toBe("一段");
    const range = document.getSelection()!.getRangeAt(0);
    range.setEnd(outside, 2);
    expect(selectedChapterText(root, document.getSelection())).toBe("");
    document.getSelection()!.collapseToStart();
    expect(selectedChapterText(root, document.getSelection())).toBe("");
  } finally { root.remove(); outside.remove(); }
});

it("renders opaque PNG text with optional metadata and rejects excessive height or length", async () => {
  const blob = await renderReaderScreenshot({ text: "选择的正文", style });
  expect(blob.type).toBe("image/png");
  const output = vi.mocked(HTMLCanvasElement.prototype.toBlob).mock.contexts[0] as HTMLCanvasElement;
  expect([output.width, output.height]).toEqual([1608, 132]);
  expect(drawn.mock.calls.map(call => call[0])).toEqual(["选择的正文"]);
  drawn.mockClear();
  await renderReaderScreenshot({ text: "正文", title: "书名", chapter: "章节", link: "https://example.com", style });
  expect(drawn.mock.calls.map(call => call[0])).toEqual(["正文", "书名 · 章节", "https://example.com"]);
  await expect(renderReaderScreenshot({ text: "字".repeat(MAX_SCREENSHOT_TEXT + 1), style })).rejects.toThrow("selection-size");
  await expect(renderReaderScreenshot({ text: "行\n".repeat(400), style })).rejects.toThrow("selection-size");
});

it("puts compact attribution below the passage and scales the final PNG without a decorative header", async () => {
  const passage = document.createElement("canvas");
  passage.width = 772;
  passage.height = 600;
  vi.spyOn(epubScreenshot, "renderEpubScreenshot").mockResolvedValue(passage);
  const ctx = HTMLCanvasElement.prototype.getContext.call(document.createElement("canvas"), "2d")!;
  const fonts: string[] = [];
  drawn.mockImplementation(() => fonts.push(ctx.font));
  try {
    await renderReaderScreenshot({ text: "正文", document: "<p>正文</p>", title: "书名", chapter: "第 13 章", style });
    expect(fonts).toEqual(["400 12px serif"]);
    expect(drawn.mock.calls).toEqual([["书名 · 第 13 章", 16, 626]]);
    expect(ctx.drawImage).toHaveBeenCalledWith(passage, 16, 16, 772, 600);
    expect(ctx.scale).toHaveBeenCalledWith(2, 2);
    expect(ctx.fillRect).toHaveBeenCalledTimes(1);
    expect(ctx.fillRect).toHaveBeenCalledWith(0, 0, 804, 660);
  } finally { drawn.mockReset(); }
});

it("loads all selected font subsets before measuring and drawing a screenshot", async () => {
  let complete!: () => void;
  const pending = new Promise<FontFace[]>(resolve => { complete = () => resolve([]); });
  const load = vi.fn(() => pending);
  const descriptor = Object.getOwnPropertyDescriptor(document, "fonts");
  Object.defineProperty(document, "fonts", { configurable: true, value: { ready: Promise.resolve(), load } });
  const ctx = HTMLCanvasElement.prototype.getContext.call(document.createElement("canvas"), "2d")!;
  const measure = vi.spyOn(ctx, "measureText");
  try {
    const render = renderReaderScreenshot({ text: "罕见的选段文字", title: "书名", chapter: "章节", link: "https://example.com", style });
    await vi.waitFor(() => expect(load).toHaveBeenCalledTimes(3));
    expect(load.mock.calls).toEqual([
      ["400 17px serif", "罕见的选段文字"], ["400 12px serif", "书名 · 章节"],
      ["400 11px serif", "https://example.com"],
    ]);
    expect(measure).not.toHaveBeenCalled();
    expect(drawn).not.toHaveBeenCalled();
    complete();
    await render;
    expect(measure).toHaveBeenCalled();
    expect(drawn.mock.calls.map(call => call[0])).toEqual(["罕见的选段文字", "书名 · 章节", "https://example.com"]);
  } finally {
    if (descriptor) Object.defineProperty(document, "fonts", descriptor);
    else Reflect.deleteProperty(document, "fonts");
  }
});

it("captures just selected TXT text and clears stale selections after chapter navigation", async () => {
  const passage = document.createElement("canvas");
  passage.width = 772; passage.height = 400;
  const rasterize = vi.spyOn(epubScreenshot, "renderEpubScreenshot").mockResolvedValue(passage);
  const chapters = [{ idx: 0, title: "第一章", linear: true }, { idx: 1, title: "第二章", linear: true }];
  const load = async (idx: number) => ({ idx, title: chapters[idx].title, content: idx ? "<p>新的正文</p>" : "<p>只选择中间文字。</p>" });
  render(<Reader title="测试书籍" chapters={chapters} loadChapter={load} />);
  const text = await screen.findByText("只选择中间文字。");
  fireEvent.click(screen.getByRole("button", { name: "选段截图" }));
  expect(screen.getByRole("status").textContent).toContain("先在正文");
  act(() => select(text.firstChild!, 1, 6));
  fireEvent.click(screen.getByRole("button", { name: "选段截图" }));
  await screen.findByAltText("所选文字截图预览");
  expect(drawn.mock.calls.map(call => call[0])).toEqual(["测试书籍 · 第一章"]);
  const snapshot = new DOMParser().parseFromString(rasterize.mock.lastCall![0], "text/html");
  expect(snapshot.body.textContent).toBe("选择中间文");
  expect(snapshot.documentElement.style.fontSize).toBe(getComputedStyle(text.closest(".reader-body")!).fontSize);
  expect(snapshot.documentElement.style.lineHeight).toBe(getComputedStyle(text.closest(".reader-body")!).lineHeight);
  expect(snapshot.head.textContent).toContain("text-align: justify");
  fireEvent.click(screen.getByRole("button", { name: "关闭" }));
  fireEvent.click(screen.getByRole("button", { name: /下一章/ }));
  await screen.findByText("新的正文");
  fireEvent.click(screen.getByRole("button", { name: "选段截图" }));
  expect(screen.queryByRole("dialog")).toBeNull();
});

it("captures styled EPUB markup without injecting publisher CSS into the app", async () => {
  const passage = document.createElement("canvas");
  passage.width = 772;
  passage.height = 600;
  const rasterize = vi.spyOn(epubScreenshot, "renderEpubScreenshot").mockResolvedValue(passage);
  const content = '<!doctype html><html><head><style>p{writing-mode:vertical-rl}</style></head><body><p>纵排正文摘录</p></body></html>';
  const { container } = render(<Reader title="测试书籍" chapters={[{ idx: 0, title: "第一章", linear: true }]} loadChapter={async () => ({ idx: 0, title: "第一章", content })} readOnly />);
  await waitFor(() => expect(container.querySelector("iframe")).not.toBeNull());
  const frame = container.querySelector("iframe")!;
  const doc = frame.contentDocument!;
  doc.head.innerHTML = "<style>p{writing-mode:vertical-rl;color:red}</style>";
  doc.body.innerHTML = '<section class="publication"><p>纵排正文摘录</p><p>不应出现在截图</p></section>';
  fireEvent.load(frame);
  act(() => select(doc.querySelector("p")!.firstChild!, 2, 6));
  fireEvent.click(screen.getByRole("button", { name: "选段截图" }));
  await screen.findByAltText("所选文字截图预览");
  expect(drawn.mock.calls.map(call => call[0])).toEqual(["测试书籍 · 第一章"]);
  const snapshot = new DOMParser().parseFromString(rasterize.mock.lastCall![0], "text/html");
  expect(snapshot.body.innerHTML).toBe('<section class="publication"><p>正文摘录</p></section>');
  expect(snapshot.head.textContent).toContain("writing-mode:vertical-rl;color:red");
  expect(document.head.textContent).not.toContain("writing-mode:vertical-rl;color:red");
});

it("captures TXT paragraph structure, typography, theme tokens, and the actual column width", () => {
  const root = document.createElement("div");
  root.className = "epub-content";
  root.style.cssText = "font:italic 500 21px/2 serif;color:rgb(67,56,42);--reader-muted:#8a7a5f;--accent:#a00";
  root.innerHTML = "<p>前文</p><p>选中<strong>强调</strong></p><p>末段</p>";
  document.body.append(root);
  vi.spyOn(root, "getBoundingClientRect").mockReturnValue(new DOMRect(0, 0, 350, 700));
  const range = document.createRange();
  range.setStart(root.children[1].firstChild!, 0);
  range.setEnd(root.children[2].firstChild!, 1);
  const selected = selectedTextContent(root, range);
  const doc = new DOMParser().parseFromString(selected.document!, "text/html");
  expect(doc.body.innerHTML).toBe('<div class="epub-content" style="' + root.getAttribute("style") + '"><p>选中<strong>强调</strong></p><p>末</p></div>');
  expect(doc.documentElement.style.fontSize).toBe("21px");
  expect(doc.documentElement.style.lineHeight).toBe("2");
  expect(doc.documentElement.style.getPropertyValue("--reader-muted")).toBe("#8a7a5f");
  expect(doc.documentElement.style.getPropertyValue("--accent")).toBe("#a00");
  expect(selected.viewport!.width).toBe(350);
  root.remove();
});

it("uses the source viewport when rasterizing and composing a selected EPUB passage", async () => {
  const passage = document.createElement("canvas");
  passage.width = 780; passage.height = 1000;
  const rasterize = vi.spyOn(epubScreenshot, "renderEpubScreenshot").mockResolvedValue(passage);
  await renderReaderScreenshot({ text: "正文", document: "<p>正文</p>", viewport: { width: 390, height: 740 }, style });
  expect(rasterize).toHaveBeenCalledWith("<p>正文</p>", 390, 12000, 740, expect.any(AbortSignal));
  const ctx = HTMLCanvasElement.prototype.getContext.call(document.createElement("canvas"), "2d")!;
  expect(ctx.drawImage).toHaveBeenCalledWith(passage, 16, 16, 390, 500);
  expect(ctx.scale).toHaveBeenCalledWith(2, 2);
  const output = vi.mocked(HTMLCanvasElement.prototype.toBlob).mock.contexts[0] as HTMLCanvasElement;
  expect([output.width, output.height]).toEqual([844, 1064]);
});

it("times out browser font loading and PNG encoding instead of leaving the preview pending", async () => {
  vi.useFakeTimers();
  const descriptor = Object.getOwnPropertyDescriptor(document, "fonts");
  Object.defineProperty(document, "fonts", { configurable: true, value: { load: vi.fn(() => new Promise(() => {})) } });
  try {
    const rendering = renderReaderScreenshot({ text: "正文", style });
    const rejected = expect(rendering).rejects.toThrow("screenshot-timeout");
    await vi.advanceTimersByTimeAsync(SCREENSHOT_TIMEOUT);
    await rejected;
    Object.defineProperty(document, "fonts", { configurable: true, value: { load: vi.fn().mockResolvedValue([]) } });
    vi.spyOn(HTMLCanvasElement.prototype, "toBlob").mockImplementation(() => {});
    const encoding = renderReaderScreenshot({ text: "正文", style });
    const failed = expect(encoding).rejects.toThrow("screenshot-timeout");
    await vi.advanceTimersByTimeAsync(SCREENSHOT_TIMEOUT);
    await failed;
    expect(vi.getTimerCount()).toBe(0);
  } finally {
    if (descriptor) Object.defineProperty(document, "fonts", descriptor);
    else Reflect.deleteProperty(document, "fonts");
  }
});
