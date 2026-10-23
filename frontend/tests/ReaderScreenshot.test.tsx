import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { Reader } from "../src/components/Reader";
import * as epubScreenshot from "../src/epubScreenshot";
import { MAX_SCREENSHOT_TEXT, renderReaderScreenshot, selectedChapterText, wrapScreenshotText } from "../src/readerScreenshot";

const style = { background: "#fff", foreground: "#111", muted: "#666", accent: "#369", fontFamily: "serif" };
const drawn = vi.fn();
beforeEach(() => {
  localStorage.clear();
  HTMLElement.prototype.scrollTo = vi.fn();
  vi.stubGlobal("ResizeObserver", class { observe() {} disconnect() {} });
  vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockReturnValue({ measureText: (s: string) => ({ width: s.length * 28 }), fillRect: vi.fn(), fillText: drawn, drawImage: vi.fn() } as unknown as CanvasRenderingContext2D);
  vi.spyOn(HTMLCanvasElement.prototype, "toBlob").mockImplementation(callback => callback(new Blob(["png"], { type: "image/png" })));
  Object.defineProperty(URL, "createObjectURL", { configurable: true, value: vi.fn(() => "blob:preview") });
  Object.defineProperty(URL, "revokeObjectURL", { configurable: true, value: vi.fn() });
  drawn.mockClear();
});
afterEach(() => { document.getSelection()?.removeAllRanges(); vi.restoreAllMocks(); vi.unstubAllGlobals(); });

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
  expect(drawn.mock.calls.map(call => call[0])).toEqual(["选择的正文"]);
  drawn.mockClear();
  await renderReaderScreenshot({ text: "正文", title: "书名", chapter: "章节", link: "https://example.com", style });
  expect(drawn.mock.calls.map(call => call[0])).toEqual(["书名", "章节", "正文", "https://example.com"]);
  await expect(renderReaderScreenshot({ text: "字".repeat(MAX_SCREENSHOT_TEXT + 1), style })).rejects.toThrow("selection-size");
  await expect(renderReaderScreenshot({ text: "行\n".repeat(300), style })).rejects.toThrow("selection-size");
});

it("keeps screenshot headings compact without resizing the EPUB passage", async () => {
  const passage = document.createElement("canvas");
  passage.width = 772;
  passage.height = 600;
  vi.spyOn(epubScreenshot, "renderEpubScreenshot").mockResolvedValue(passage);
  const ctx = HTMLCanvasElement.prototype.getContext.call(document.createElement("canvas"), "2d")!;
  const fonts: string[] = [];
  drawn.mockImplementation(() => fonts.push(ctx.font));
  try {
    await renderReaderScreenshot({ text: "正文", document: "<p>正文</p>", title: "书名", chapter: "第 13 章", style });
    expect(fonts).toEqual(["600 20px serif", "400 18px serif"]);
    expect(drawn.mock.calls).toEqual([["书名", 64, 96], ["第 13 章", 64, 150]]);
    expect(ctx.drawImage).toHaveBeenCalledWith(passage, 64, 202, 772, 600);
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
    await vi.waitFor(() => expect(load).toHaveBeenCalledTimes(4));
    expect(load.mock.calls).toEqual([
      ["600 20px serif", "书名"], ["400 18px serif", "章节"],
      ["400 28px serif", "罕见的选段文字"], ["400 20px serif", "https://example.com"],
    ]);
    expect(measure).not.toHaveBeenCalled();
    expect(drawn).not.toHaveBeenCalled();
    complete();
    await render;
    expect(measure).toHaveBeenCalled();
    expect(drawn.mock.calls.map(call => call[0])).toEqual(["书名", "章节", "罕见的选段文字", "https://example.com"]);
  } finally {
    if (descriptor) Object.defineProperty(document, "fonts", descriptor);
    else Reflect.deleteProperty(document, "fonts");
  }
});

it("captures just selected TXT text and clears stale selections after chapter navigation", async () => {
  const chapters = [{ idx: 0, title: "第一章", linear: true }, { idx: 1, title: "第二章", linear: true }];
  const load = async (idx: number) => ({ idx, title: chapters[idx].title, content: idx ? "<p>新的正文</p>" : "<p>只选择中间文字。</p>" });
  render(<Reader title="测试书籍" chapters={chapters} loadChapter={load} />);
  const text = await screen.findByText("只选择中间文字。");
  fireEvent.click(screen.getByRole("button", { name: "选段截图" }));
  expect(screen.getByRole("status").textContent).toContain("先在正文");
  act(() => select(text.firstChild!, 1, 6));
  fireEvent.click(screen.getByRole("button", { name: "选段截图" }));
  await screen.findByAltText("所选文字截图预览");
  expect(drawn.mock.calls.map(call => call[0])).toEqual(["测试书籍", "第一章", "选择中间文"]);
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
  expect(drawn.mock.calls.map(call => call[0])).toEqual(["测试书籍", "第一章"]);
  const snapshot = new DOMParser().parseFromString(rasterize.mock.lastCall![0], "text/html");
  expect(snapshot.body.innerHTML).toBe('<section class="publication"><p>正文摘录</p></section>');
  expect(snapshot.head.textContent).toContain("writing-mode:vertical-rl;color:red");
  expect(document.head.textContent).not.toContain("writing-mode:vertical-rl;color:red");
});
