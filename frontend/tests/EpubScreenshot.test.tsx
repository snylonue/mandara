import { fireEvent } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { rasterizeScreenshot } from "../src/screenshotImage";
import { renderEpubScreenshot } from "../src/epubScreenshot";
import { MAX_SCREENSHOT_TEXT, selectedEpubContent } from "../src/readerScreenshot";
import { SCREENSHOT_TIMEOUT } from "../src/screenshotTask";

vi.mock("../src/screenshotImage", async importOriginal => ({ ...await importOriginal<typeof import("../src/screenshotImage")>(), rasterizeScreenshot: vi.fn() }));
afterEach(() => { document.querySelectorAll("iframe").forEach(frame => frame.remove()); vi.unstubAllGlobals(); vi.restoreAllMocks(); vi.clearAllMocks(); vi.mocked(rasterizeScreenshot).mockReset(); vi.useRealTimers(); });

function chapter() {
  const frame = document.createElement("iframe");
  document.body.append(frame);
  return frame.contentDocument!;
}

it("keeps publication styles, ancestor attributes, ruby and inline formatting only for the selected range", () => {
  const doc = chapter();
  doc.documentElement.lang = "zh-CN";
  Object.defineProperties(doc.documentElement, { clientWidth: { value: 390 }, clientHeight: { value: 740 } });
  doc.head.innerHTML = '<style>@font-face{font-family:book;src:url(data:font/woff2;base64,AA==)} body.book{writing-mode:vertical-rl} #chapter .passage{color:red;text-align:center}</style>';
  doc.body.className = "book";
  doc.body.innerHTML = '<section id="chapter"><p>未选择的前文</p><p class="passage">开头<strong>强调</strong><ruby>汉字<rt>かんじ</rt></ruby>结尾</p><p>未选择的后文</p></section>';
  const passage = doc.querySelector(".passage")!;
  const range = doc.createRange();
  range.setStart(passage.firstChild!, 1);
  range.setEnd(passage.lastChild!, 1);
  doc.getSelection()!.addRange(range);
  const excerpt = selectedEpubContent(doc, doc.getSelection());
  expect(excerpt.viewport).toEqual({ width: 390, height: 740 });
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
  for (const [target, original] of [[doc.documentElement, parsed.documentElement], [doc.body, parsed.body]]) {
    for (const attribute of Array.from(original.attributes)) target.setAttribute(attribute.name, attribute.value);
  }
  Object.defineProperties(doc.documentElement, { scrollWidth: { value: width }, scrollHeight: { value: height } });
  vi.spyOn(doc.documentElement, "getBoundingClientRect").mockReturnValue(new DOMRect(0, 0, width, height));
  fireEvent.load(frame);
  return { frame, doc };
}
const source = '<!doctype html><html><head><style>@font-face{font-family:book;src:url(data:font/woff2;base64,AA==)} p{font-family:book;color:red}</style></head><body><p>正文</p></body></html>';

it("rasterizes in a sandbox with embedded font CSS and removes the temporary frame", async () => {
  const canvas = document.createElement("canvas");
  vi.mocked(rasterizeScreenshot).mockResolvedValueOnce(canvas);
  const rendering = renderEpubScreenshot(source, 772, 12000);
  const { frame, doc } = await loadCapture();
  expect(frame.getAttribute("sandbox")).toBe("allow-same-origin");
  expect(frame.srcdoc).toContain("script-src 'none'");
  expect(await rendering).toBe(canvas);
  expect(rasterizeScreenshot).toHaveBeenCalledWith(doc.documentElement, expect.objectContaining({ width: 772, height: 400, pixelRatio: 2, fontEmbedCSS: expect.stringContaining("@font-face") }));
  expect(document.body.contains(frame)).toBe(false);
  expect(document.head.textContent).not.toContain("font-family:book");
});

it.each([false, true])("serializes EPUB semantic attributes as valid XML with an existing declaration: %s", async existing => {
  const namespace = "http://www.idpf.org/2007/ops";
  const chapter = '<!doctype html><html' + (existing ? ' xmlns:epub="' + namespace + '"' : '') + '><head><style>section{color:red}</style></head><body epub:type="bodymatter"><section epub:type="chapter"><p>所选正文</p></section></body></html>';
  const canvas = document.createElement("canvas");
  vi.mocked(rasterizeScreenshot).mockImplementationOnce(async node => {
    expect(node.querySelector("section")!.getAttribute("epub:type")).toBe("chapter");
    expect(getComputedStyle(node.querySelector("section")!).color).toBe("rgb(255, 0, 0)");
    const xml = new XMLSerializer().serializeToString(node);
    const parsed = new DOMParser().parseFromString(xml, "application/xhtml+xml");
    expect(parsed.querySelector("parsererror")).toBeNull();
    expect(parsed.querySelector("section")!.getAttributeNS(namespace, "type")).toBe("chapter");
    expect(parsed.querySelector("body")!.getAttributeNS(namespace, "type")).toBe("bodymatter");
    return canvas;
  });
  const rendering = renderEpubScreenshot(chapter, 390, 12000);
  await loadCapture(390, 400);
  expect(await rendering).toBe(canvas);
});

it("allows and embeds self-hosted reader fonts before rasterizing", async () => {
  vi.mocked(rasterizeScreenshot).mockResolvedValueOnce(document.createElement("canvas"));
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
  expect(fetchFont).toHaveBeenCalledWith(location.origin + "/assets/reader.woff2", { signal: expect.any(AbortSignal) });
  const options = vi.mocked(rasterizeScreenshot).mock.lastCall![1]!;
  expect(options.fontEmbedCSS).toContain("data:font/woff2;base64,Zm9udA==");
  expect(options.fontEmbedCSS).not.toContain("/assets/reader.woff2");
});

it("limits raster pixels for a long excerpt without changing its CSS dimensions", async () => {
  vi.mocked(rasterizeScreenshot).mockResolvedValueOnce(document.createElement("canvas"));
  const rendering = renderEpubScreenshot(source, 772, 12000);
  await loadCapture(772, 11000);
  await rendering;
  const options = vi.mocked(rasterizeScreenshot).mock.lastCall![1]!;
  expect([options.width, options.height]).toEqual([772, 11000]);
  expect(options.pixelRatio).toBeLessThan(2);
  expect(Math.floor(772 * options.pixelRatio!) * Math.floor(11000 * options.pixelRatio!)).toBeLessThanOrEqual(16000000);
});

it("loads book illustrations without waiting for a pending SVG decode", async () => {
  const canvas = document.createElement("canvas");
  vi.mocked(rasterizeScreenshot).mockResolvedValueOnce(canvas);
  const rendering = renderEpubScreenshot(source.replace("</p>", '<img src="data:image/svg+xml,%3Csvg%2F%3E"></p>'), 390, 12000);
  const loaded = loadCapture(390, 400);
  const image = document.querySelector<HTMLIFrameElement>('iframe[aria-hidden="true"]')!.contentDocument!.querySelector("img")!;
  Object.defineProperties(image, { complete: { value: true }, naturalWidth: { value: 16 } });
  image.decode = vi.fn(() => new Promise(() => {}));
  await loaded;
  expect(await rendering).toBe(canvas);
  expect(image.decode).not.toHaveBeenCalled();
  expect(image.loading).toBe("eager");
});

it.each(["href", "xlink:href", "both"])("embeds SVG %s image references before cloning without detached load events", async attribute => {
  const attributes = attribute === "both" ? 'href="/api/images/note" xlink:href="/api/images/note"' : attribute + '="/api/images/note"';
  const picture = '<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="20" height="20"><image width="20" height="20" ' + attributes + '/><image width="20" height="20" href="/api/images/note"/></svg>';
  const fetchImage = vi.fn().mockResolvedValue({ ok: true, blob: async () => new Blob(["image"], { type: "image/png" }) });
  vi.stubGlobal("fetch", fetchImage);
  const canvas = document.createElement("canvas");
  vi.mocked(rasterizeScreenshot).mockImplementationOnce(async node => {
    for (const image of node.querySelectorAll("svg image")) {
      const reference = image.getAttribute("href") || image.getAttributeNS("http://www.w3.org/1999/xlink", "href");
      expect(reference).toMatch(/^data:image\/png;base64,/);
    }
    const xml = new XMLSerializer().serializeToString(node);
    expect(new DOMParser().parseFromString(xml, "application/xhtml+xml").querySelector("parsererror")).toBeNull();
    return canvas;
  });
  const rendering = renderEpubScreenshot(source.replace("</p>", picture + "</p>"), 390, 12000);
  await loadCapture(390, 400);
  expect(await rendering).toBe(canvas);
  expect(fetchImage).toHaveBeenCalledExactlyOnceWith(location.origin + "/api/images/note", { signal: expect.any(AbortSignal) });
});

it("rejects excessive dimensions without truncating or allocating a canvas", async () => {
  const rendering = renderEpubScreenshot(source, 772, 12000);
  const rejected = expect(rendering).rejects.toThrow("selection-size");
  const { frame } = await loadCapture(772, 12001);
  await rejected;
  expect(rasterizeScreenshot).not.toHaveBeenCalled();
  expect(document.body.contains(frame)).toBe(false);
});

it("includes leftward vertical overflow without changing the measured layout", async () => {
  vi.mocked(rasterizeScreenshot).mockResolvedValueOnce(document.createElement("canvas"));
  const rendering = renderEpubScreenshot(source, 772, 12000);
  const loaded = loadCapture();
  const frame = document.querySelector<HTMLIFrameElement>('iframe[aria-hidden="true"]')!;
  vi.spyOn(frame.contentDocument!.querySelector("p")!, "getBoundingClientRect").mockReturnValue(new DOMRect(-300, 0, 500, 400));
  await loaded;
  await rendering;
  expect(rasterizeScreenshot).toHaveBeenCalledWith(expect.anything(), expect.objectContaining({ width: 1072, style: expect.objectContaining({ width: "772px", transform: "translate(300px,0px)" }) }));
});

it("removes the temporary frame if rasterization fails", async () => {
  vi.mocked(rasterizeScreenshot).mockRejectedValueOnce(new Error("render-failed"));
  const rendering = renderEpubScreenshot(source, 772, 12000);
  const rejected = expect(rendering).rejects.toThrow("render-failed");
  const { frame } = await loadCapture();
  await rejected;
  expect(document.body.contains(frame)).toBe(false);
});

it("embeds only used families and Unicode subsets, with deduplicated parallel requests", async () => {
  const faces = [
    ['book', '/assets/zh.woff2', 'U+6B63,U+6587'],
    ['book', '/assets/zh.woff2', 'U+6B??'],
    ['book', '/assets/other.woff2', 'U+0000-00FF'],
    ['unused', '/assets/unused.woff2', 'U+0-10FFFF'],
    ['book', '/assets/wen.woff2', 'U+6587'],
  ];
  const css = faces.map(([family, url, range]) => '@font-face{font-family:' + family + ';src:url("' + url + '");unicode-range:' + range + '}').join('');
  let finish!: () => void;
  const response = new Promise(resolve => { finish = () => resolve({ ok: true, blob: async () => new Blob(["font"], { type: "font/woff2" }) }); });
  const fetchFont = vi.fn(() => response);
  vi.stubGlobal("fetch", fetchFont);
  vi.mocked(rasterizeScreenshot).mockResolvedValueOnce(document.createElement("canvas"));
  const rendering = renderEpubScreenshot(source.replace(/<style>[\s\S]*?<\/style>/, '<style>' + css + 'p{font-family:book}</style>'), 390, 12000);
  const loaded = loadCapture(390, 400);
  const doc = document.querySelector<HTMLIFrameElement>('iframe[aria-hidden="true"]')!.contentDocument!;
  faces.forEach(([family, url, range], i) => {
    const rule = doc.styleSheets[0].cssRules[i] as CSSFontFaceRule;
    // jsdom drops src and unicode-range descriptors; retain browser CSSOM behavior.
    Object.defineProperty(rule, "cssText", {
      value: '@font-face{font-family:' + family + ';src:url("' + url + '");unicode-range:' + range + '}',
    });
    const original = rule.style.getPropertyValue.bind(rule.style);
    vi.spyOn(rule.style, "getPropertyValue").mockImplementation(name => name === "unicode-range" ? range : original(name));
  });
  await loaded;
  try {
    await vi.waitFor(() => expect(fetchFont).toHaveBeenCalledTimes(2));
    expect(fetchFont.mock.calls.map(call => call[0])).toEqual([location.origin + '/assets/zh.woff2', location.origin + '/assets/wen.woff2']);
    expect(rasterizeScreenshot).not.toHaveBeenCalled();
  } finally {
    finish();
    await rendering;
  }
  const embedded = vi.mocked(rasterizeScreenshot).mock.lastCall![1]!.fontEmbedCSS;
  expect(embedded).toContain('U+6B??');
  expect(embedded).not.toContain('unused');
  expect(embedded).not.toContain('U+0000-00FF');
});

it("preserves the original reading viewport instead of using a fixed capture height", async () => {
  vi.mocked(rasterizeScreenshot).mockResolvedValueOnce(document.createElement("canvas"));
  const rendering = renderEpubScreenshot(source, 390, 12000, 740);
  const { frame } = await loadCapture(390, 400);
  expect(frame.style.width).toBe("390px");
  expect(frame.style.height).toBe("740px");
  await rendering;
  expect(rasterizeScreenshot).toHaveBeenCalledWith(expect.anything(), expect.objectContaining({ width: 390 }));
});

it("times out stalled rasterization and removes its isolated document", async () => {
  vi.useFakeTimers();
  vi.mocked(rasterizeScreenshot).mockImplementationOnce(() => new Promise(() => {}));
  const rendering = renderEpubScreenshot(source, 772, 12000);
  const rejected = expect(rendering).rejects.toThrow("screenshot-timeout");
  const { frame } = await loadCapture();
  await vi.advanceTimersByTimeAsync(1);
  expect(rasterizeScreenshot).toHaveBeenCalled();
  await vi.advanceTimersByTimeAsync(SCREENSHOT_TIMEOUT);
  await rejected;
  expect(frame.isConnected).toBe(false);
  expect(vi.getTimerCount()).toBe(0);
});

it("cancels pending frame loads without waiting for the timeout", async () => {
  const controller = new AbortController();
  const rendering = renderEpubScreenshot(source, 772, 12000, 600, controller.signal);
  const rejected = expect(rendering).rejects.toThrow("cancelled");
  const frame = document.querySelector('iframe[aria-hidden="true"]')!;
  controller.abort(new Error("cancelled"));
  await rejected;
  expect(frame.isConnected).toBe(false);
  expect(rasterizeScreenshot).not.toHaveBeenCalled();
});
