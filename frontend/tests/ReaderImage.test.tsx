import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { readerImageAt } from "../src/readerImage";
import { ReaderImageDialog } from "../src/components/ReaderImageDialog";
import { Reader } from "../src/components/Reader";

const image = { src: location.origin + "/api/images/illustration", alt: "彩色插图" };
const blob = new Blob(["original JPEG bytes"], { type: "image/jpeg" });
const share = vi.fn();
const canShare = vi.fn();
const fetchImage = vi.fn();

beforeEach(() => {
  vi.clearAllMocks();
  vi.stubGlobal("isSecureContext", true);
  vi.stubGlobal("fetch", fetchImage);
  Object.defineProperty(navigator, "share", { configurable: true, value: share });
  Object.defineProperty(navigator, "canShare", { configurable: true, value: canShare });
  share.mockReset().mockResolvedValue(undefined);
  canShare.mockReset().mockReturnValue(true);
  fetchImage.mockReset().mockResolvedValue({ ok: true, blob: async () => blob });
  localStorage.clear();
  document.getSelection()?.removeAllRanges();
  HTMLElement.prototype.scrollTo = vi.fn();
  vi.stubGlobal("ResizeObserver", class { observe() {} disconnect() {} });
});
afterEach(() => { vi.unstubAllGlobals(); vi.restoreAllMocks(); });

it.each([
  '<img src="/api/images/illustration" alt="彩色插图">',
  '<svg><title>彩色插图</title><image href="/api/images/illustration" /></svg>',
  '<svg xmlns:xlink="http://www.w3.org/1999/xlink"><title>彩色插图</title><image xlink:href="/api/images/illustration" /></svg>',
])("resolves original HTML and SVG illustrations: %s", markup => {
  const root = document.createElement("div");
  root.innerHTML = markup;
  expect(readerImageAt(root.querySelector("img,image")!)).toEqual(image);
  if (root.querySelector("svg")) expect(readerImageAt(root.querySelector("svg")!)).toEqual(image);
  expect(root.innerHTML).not.toContain("data-mandara");
});

it.each([
  '<img class="gaiji" src="/api/images/glyph">',
  '<a href="epub:chapter/2"><img src="/api/images/illustration"></a>',
  '<img src="https://external.test/image.jpg">',
  '<img src="/api/books/private">',
  '<img src="javascript:alert(1)">',
  '<img src="data:text/html,test">',
  '<img>',
  '<svg><path d="M0 0"/></svg>',
  '<svg><image href="/api/images/one"/><image href="/api/images/two"/></svg>',
])("leaves glyphs, navigation and unsupported resources alone: %s", markup => {
  const root = document.createElement("div");
  root.innerHTML = markup;
  expect(readerImageAt(root.querySelector("img,svg")!)).toBeNull();
});

it("accepts embedded image data and the displayed responsive image", () => {
  const img = document.createElement("img");
  img.src = "data:image/png;base64,cG5n";
  expect(readerImageAt(img)?.src).toBe(img.src);
  Object.defineProperty(img, "currentSrc", { value: image.src });
  expect(readerImageAt(img)?.src).toBe(image.src);
});

it("shows a native image outside the frame without swallowing long-press events", () => {
  render(<ReaderImageDialog image={image} onClose={vi.fn()} />);
  const preview = screen.getByRole("img", { name: image.alt });
  expect(preview.getAttribute("src")).toBe(image.src);
  expect(preview.ownerDocument).toBe(document);
  expect(preview.closest("iframe")).toBeNull();
  expect(fireEvent.contextMenu(preview)).toBe(true);
  expect(fireEvent.touchStart(preview)).toBe(true);
  const original = screen.getByRole("link", { name: "打开原图" });
  expect(original.getAttribute("href")).toBe(image.src);
  expect(original.getAttribute("target")).toBe("_blank");
});

it("shares only the prepared original file synchronously from the button click", async () => {
  render(<ReaderImageDialog image={image} onClose={vi.fn()} />);
  const button = screen.getByRole("button", { name: "系统分享" }) as HTMLButtonElement;
  await waitFor(() => expect(button.disabled).toBe(false));
  expect(share).not.toHaveBeenCalled();
  expect(fetchImage).toHaveBeenCalledTimes(1);
  fireEvent.click(button);
  expect(share).toHaveBeenCalledTimes(1);
  const payload = share.mock.calls[0][0];
  expect(Object.keys(payload)).toEqual(["files"]);
  expect(payload.files).toHaveLength(1);
  expect(payload.files[0]).toBeInstanceOf(File);
  expect(payload.files[0].name).toBe("书内插图.jpg");
  expect(payload.files[0].type).toBe("image/jpeg");
  expect(payload.files[0].size).toBe(blob.size);
  await waitFor(() => expect(button.disabled).toBe(false));
});

it("keeps the original available on HTTP without fetching for unsupported sharing", () => {
  vi.stubGlobal("isSecureContext", false);
  render(<ReaderImageDialog image={image} onClose={vi.fn()} />);
  expect(screen.queryByRole("button", { name: "系统分享" })).toBeNull();
  expect(screen.getByRole("link", { name: "打开原图" })).toBeTruthy();
  expect(screen.getByText(/长按图片可/)).toBeTruthy();
  expect(fetchImage).not.toHaveBeenCalled();
});

it.each(["missing", "file-unsupported", "fetch-failed"])("preserves long-press fallback when sharing is %s", async failure => {
  if (failure === "missing") Object.defineProperty(navigator, "canShare", { configurable: true, value: undefined });
  if (failure === "file-unsupported") canShare.mockReturnValue(false);
  if (failure === "fetch-failed") fetchImage.mockRejectedValue(new Error("network"));
  render(<ReaderImageDialog image={image} onClose={vi.fn()} />);
  await act(async () => {});
  expect(screen.getByRole("img", { name: image.alt })).toBeTruthy();
  const button = screen.queryByRole("button", { name: "系统分享" }) as HTMLButtonElement | null;
  expect(!button || button.disabled).toBe(true);
  expect(screen.getByText(/若系统分享按钮不可用/)).toBeTruthy();
  expect(share).not.toHaveBeenCalled();
});

it("ignores cancellation but reports share failure and allows retry", async () => {
  share.mockRejectedValueOnce(new DOMException("cancelled", "AbortError"));
  render(<ReaderImageDialog image={image} onClose={vi.fn()} />);
  const button = screen.getByRole("button", { name: "系统分享" }) as HTMLButtonElement;
  await waitFor(() => expect(button.disabled).toBe(false));
  fireEvent.click(button);
  await waitFor(() => expect(button.disabled).toBe(false));
  expect(screen.queryByRole("alert")).toBeNull();
  share.mockRejectedValueOnce(new Error("denied"));
  fireEvent.click(button);
  expect((await screen.findByRole("alert")).textContent).toContain("分享失败");
  fireEvent.click(button);
  await waitFor(() => expect(screen.queryByRole("alert")).toBeNull());
});

it("does not dismiss or submit again while the native share sheet is open", async () => {
  let finish!: () => void;
  share.mockImplementationOnce(() => new Promise<void>(resolve => { finish = resolve; }));
  const close = vi.fn();
  render(<ReaderImageDialog image={image} onClose={close} />);
  const button = screen.getByRole("button", { name: "系统分享" }) as HTMLButtonElement;
  await waitFor(() => expect(button.disabled).toBe(false));
  fireEvent.click(button);
  fireEvent.click(screen.getByRole("button", { name: "分享中…" }));
  fireEvent.click(screen.getByRole("button", { name: "关闭" }));
  expect(share).toHaveBeenCalledTimes(1);
  expect(close).not.toHaveBeenCalled();
  await act(async () => { finish(); });
  fireEvent.click(screen.getByRole("button", { name: "关闭" }));
  expect(close).toHaveBeenCalledTimes(1);
});

it("aborts pending file preparation when the viewer closes", async () => {
  let resolve!: (response: unknown) => void;
  fetchImage.mockImplementationOnce(() => new Promise(done => { resolve = done; }));
  const { unmount } = render(<ReaderImageDialog image={image} onClose={vi.fn()} />);
  const signal = fetchImage.mock.calls[0][1].signal as AbortSignal;
  unmount();
  expect(signal.aborted).toBe(true);
  await act(async () => { resolve({ ok: true, blob: async () => blob }); });
  expect(canShare).not.toHaveBeenCalled();
});

it.each([false, true])("opens illustrations from the reader with isolated EPUB=%s without changing the chapter", async isolated => {
  const markup = '<p>正文</p><img src="/api/images/illustration" alt="彩色插图">';
  const content = isolated ? '<!doctype html><html><body>' + markup + '</body></html>' : markup;
  const loadChapter = vi.fn().mockResolvedValue({ idx: 0, title: "第一章", content });
  const { container } = render(<Reader title="测试书名" chapters={[{ idx: 0, title: "第一章" }]} loadChapter={loadChapter} />);
  await waitFor(() => expect(container.querySelector(isolated ? "iframe" : ".epub-content img")).not.toBeNull());
  const frame = container.querySelector("iframe");
  const doc = frame?.contentDocument ?? document;
  if (frame) {
    doc.body.innerHTML = markup;
    fireEvent.load(frame);
  }
  const illustration = isolated ? doc.querySelector("img")! : container.querySelector(".epub-content img")!;
  fireEvent.click(illustration);
  const dialog = await screen.findByRole("dialog", { name: "书内插图" });
  expect(dialog.querySelector("img")!.src).toBe(image.src);
  expect(illustration.isConnected).toBe(true);
  fireEvent.click(screen.getByRole("button", { name: "关闭" }));
  expect(screen.queryByRole("dialog")).toBeNull();
  expect(loadChapter).toHaveBeenCalledTimes(1);
});
