import { fireEvent } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { toCanvas } from "html-to-image";
import { renderEpubScreenshot } from "../src/epubScreenshot";
import { MAX_SCREENSHOT_TEXT, selectedEpubContent } from "../src/readerScreenshot";

vi.mock("html-to-image", () => ({ toCanvas: vi.fn() }));
afterEach(() => { document.querySelectorAll("iframe").forEach(frame => frame.remove()); vi.unstubAllGlobals(); vi.restoreAllMocks(); vi.clearAllMocks(); });

function chapter() {
  const frame = document.createElement("iframe");
  document.body.append(frame);
  return frame.contentDocument!;
}

it("keeps publication styles, ancestor attributes, ruby and inline formatting only for the selected range", () => {
  const doc = chapter();
  doc.documentElement.lang = "zh-CN";
  doc.head.innerHTML = '<style>@font-face{font-family:book;src:url(data:font/woff2;base64,AA==)} body.book{writing-mode:vertical-rl} #chapter .passage{color:red;text-align:center}</style>';
  doc.body.className = "book";
  doc.body.innerHTML = '<section id="chapter"><p>未选择的前文</p><p class="passage">开头<strong>强调</strong><ruby>汉字<rt>かんじ</rt></ruby>结尾</p><p>未选择的后文</p></section>';
  const passage = doc.querySelector(".passage")!;
  const range = doc.createRange();
  range.setStart(passage.firstChild!, 1);
  range.setEnd(passage.lastChild!, 1);
  doc.getSelection()!.addRange(range);
  const excerpt = selectedEpubContent(doc, doc.getSelection());
  const snapshot = new DOMParser().parseFromString(excerpt.document!, "text/html");
  expect(snapshot.documentElement.lang).toBe("zh-CN");
  expect(snapshot.body.className).toBe("book");
  expect(snapshot.head.textContent).toBe(doc.head.textContent);
  expect(snapshot.body.innerHTML).toBe('<section id="chapter"><p class="passage">头<strong>强调</strong><ruby>汉字<rt>かんじ</rt></ruby>结</p></section>');
  expect(snapshot.body.textContent).toBe(excerpt.text);
  expect(doc.body.textContent).toContain("未选择的前文");
  doc.body.innerHTML = "";
  expect(excerpt.document).toContain("强调");
});

it("does not copy active markup, event handlers or source document policies", () => {
  const doc = chapter();
  doc.head.innerHTML = '<base href="https://example.com"><meta http-equiv="refresh" content="0"><script>bad()</script><style>p{color:red}</style>';
  doc.body.innerHTML = '<p onclick="bad()">正文<img onerror="bad()" src="/api/images/glyph"><script>bad()</script><iframe srcdoc="bad"></iframe></p>';
  const range = doc.createRange(); range.selectNodeContents(doc.body);
  doc.getSelection()!.addRange(range);
  const snapshot = new DOMParser().parseFromString(selectedEpubContent(doc, doc.getSelection()).document!, "text/html");
  expect(snapshot.querySelector("script,iframe,base,meta,[onclick],[onerror]")).toBeNull();
  expect(snapshot.querySelector("img")!.getAttribute("src")).toBe("/api/images/glyph");
  expect(snapshot.head.querySelector("style")!.textContent).toBe("p{color:red}");
});

it("does not snapshot empty or oversized selections", () => {
  const doc = chapter();
  expect(selectedEpubContent(doc, null)).toEqual({ text: "" });
  doc.body.textContent = "字".repeat(MAX_SCREENSHOT_TEXT + 1);
  const range = doc.createRange(); range.selectNodeContents(doc.body);
  doc.getSelection()!.addRange(range);
  expect(selectedEpubContent(doc, doc.getSelection()).document).toBeUndefined();
});

async function loadCapture(width = 772, height = 400) {
  const frame = document.querySelector<HTMLIFrameElement>('iframe[aria-hidden="true"]')!;
  const doc = frame.contentDocument!;
  const parsed = new DOMParser().parseFromString(frame.srcdoc, "text/html");
  doc.head.innerHTML = parsed.head.innerHTML;
  doc.body.innerHTML = parsed.body.innerHTML;
  Object.defineProperties(doc.documentElement, { scrollWidth: { value: width }, scrollHeight: { value: height } });
  vi.spyOn(doc.documentElement, "getBoundingClientRect").mockReturnValue(new DOMRect(0, 0, width, height));
  fireEvent.load(frame);
  return { frame, doc };
}
const source = '<!doctype html><html><head><style>@font-face{font-family:book;src:url(data:font/woff2;base64,AA==)} p{font-family:book;color:red}</style></head><body><p>正文</p></body></html>';

it("rasterizes in a sandbox with embedded font CSS and removes the temporary frame", async () => {
  const canvas = document.createElement("canvas");
  vi.mocked(toCanvas).mockResolvedValueOnce(canvas);
  const rendering = renderEpubScreenshot(source, 772, 12000);
  const { frame, doc } = await loadCapture();
  expect(frame.getAttribute("sandbox")).toBe("allow-same-origin");
  expect(frame.srcdoc).toContain("script-src 'none'");
  expect(await rendering).toBe(canvas);
  expect(toCanvas).toHaveBeenCalledWith(doc.documentElement, expect.objectContaining({ width: 772, height: 400, pixelRatio: 1, fontEmbedCSS: expect.stringContaining("@font-face") }));
  expect(document.body.contains(frame)).toBe(false);
  expect(document.head.textContent).not.toContain("font-family:book");
});

it("allows and embeds self-hosted reader fonts before rasterizing", async () => {
  vi.mocked(toCanvas).mockResolvedValueOnce(document.createElement("canvas"));
  const fetchFont = vi.fn().mockResolvedValue({ ok: true, blob: async () => new Blob(["font"], { type: "font/woff2" }) });
  vi.stubGlobal("fetch", fetchFont);
  const rendering = renderEpubScreenshot(source.replace("data:font/woff2;base64,AA==", "/assets/reader.woff2"), 772, 12000);
  const loaded = loadCapture();
  const capture = document.querySelector<HTMLIFrameElement>('iframe[aria-hidden="true"]')!;
  // jsdom drops the font-face src descriptor from CSSOM serialization.
  Object.defineProperty(capture.contentDocument!.styleSheets[0].cssRules[0], "cssText", {
    value: '@font-face{font-family:book;src:url("/assets/reader.woff2")}',
  });
  const { frame } = await loaded;
  expect(frame.srcdoc).toContain("font-src data: " + location.origin);
  await rendering;
  expect(fetchFont).toHaveBeenCalledWith(location.origin + "/assets/reader.woff2");
  const options = vi.mocked(toCanvas).mock.lastCall![1]!;
  expect(options.fontEmbedCSS).toContain("data:font/woff2;base64,Zm9udA==");
  expect(options.fontEmbedCSS).not.toContain("/assets/reader.woff2");
});

it("rejects excessive dimensions without truncating or allocating a canvas", async () => {
  const rendering = renderEpubScreenshot(source, 772, 12000);
  const rejected = expect(rendering).rejects.toThrow("selection-size");
  const { frame } = await loadCapture(772, 12001);
  await rejected;
  expect(toCanvas).not.toHaveBeenCalled();
  expect(document.body.contains(frame)).toBe(false);
});

it("includes leftward vertical overflow without changing the measured layout", async () => {
  vi.mocked(toCanvas).mockResolvedValueOnce(document.createElement("canvas"));
  const rendering = renderEpubScreenshot(source, 772, 12000);
  const loaded = loadCapture();
  const frame = document.querySelector<HTMLIFrameElement>('iframe[aria-hidden="true"]')!;
  vi.spyOn(frame.contentDocument!.querySelector("p")!, "getBoundingClientRect").mockReturnValue(new DOMRect(-300, 0, 500, 400));
  await loaded;
  await rendering;
  expect(toCanvas).toHaveBeenCalledWith(expect.anything(), expect.objectContaining({ width: 1072, style: expect.objectContaining({ width: "772px", transform: "translate(300px,0px)" }) }));
});

it("removes the temporary frame if rasterization fails", async () => {
  vi.mocked(toCanvas).mockRejectedValueOnce(new Error("render-failed"));
  const rendering = renderEpubScreenshot(source, 772, 12000);
  const rejected = expect(rendering).rejects.toThrow("render-failed");
  const { frame } = await loadCapture();
  await rejected;
  expect(document.body.contains(frame)).toBe(false);
});
