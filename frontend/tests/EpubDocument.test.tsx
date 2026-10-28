import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { act, fireEvent, render } from "@testing-library/react";
import { createRef } from "react";
import { readFileSync } from "node:fs";
import { EpubDocument, type EpubDocumentHandle } from "../src/components/EpubDocument";
import type { ReaderSettings } from "../src/components/ReaderSettings";

const settings: ReaderSettings = { fontSize: 20, lineHeight: 2, theme: "light", font: "serif", widthEm: 42 };
const props = { title: "正文", settings, onFraction: vi.fn(), onReady: vi.fn(), onNavigate: vi.fn() };

beforeEach(() => {
  vi.stubGlobal("ResizeObserver", class {
    observe() {}
    disconnect() {}
  });
});
afterEach(() => vi.unstubAllGlobals());

function load(content: string, onImageOpen = vi.fn()) {
  const view = render(<EpubDocument {...props} content={content} onImageOpen={onImageOpen} />);
  const frame = view.container.querySelector("iframe")!;
  const doc = frame.contentDocument!;
  // JSDOM does not load srcdoc. Populate the existing frame document, keeping
  // the defaults that React may have already installed in its initial effect.
  const parsed = new DOMParser().parseFromString(frame.srcdoc, "text/html");
  for (const attr of parsed.documentElement.attributes) {
    doc.documentElement.setAttribute(attr.name, attr.value);
  }
  for (const child of Array.from(parsed.head.childNodes)) doc.head.appendChild(doc.importNode(child, true));
  doc.body.innerHTML = parsed.body.innerHTML;
  for (const attr of parsed.body.attributes) doc.body.setAttribute(attr.name, attr.value);
  fireEvent.load(frame);
  return { ...view, frame, doc };
}
it("provides Chinese serif subsets inside Japanese-language publications without rewriting their styles", () => {
  const text = "看着路边陈旧的扬声器里轰鸣播放着音乐的样子，拉奇皱起了眉头。";
  const publisher = "p{font-weight:400}.original{opacity:0.4}";
  const { doc } = load(`<!doctype html><html lang="ja"><head><style>${publisher}</style></head>
    <body lang="ja"><p>${text}</p><p class="original"><ruby>道<rt>みち</rt></ruby></p></body></html>`);
  // JSDOM does not retain all @font-face descriptors in CSSOM.
  const css = doc.head.querySelector("style")!.textContent!;
  const faces = Array.from(css.matchAll(/@font-face\s*\{([^}]+)\}/g), match => match[1]);
  const family = '"Noto Serif SC Variable"';
  expect(faces.length).toBeGreaterThan(1);
  const tokens = readFileSync("src/styles/tokens.css", "utf8");
  expect(tokens).toContain("--font-serif: " + family + ",");
  expect(readFileSync("src/styles/index.css", "utf8")).toContain('@import "@fontsource-variable/noto-serif-sc/index.css"');
  for (const face of faces) {
    expect(face).toMatch(/font-family:\s*["']Noto Serif SC Variable["']/);
    expect(face).toMatch(/font-weight:\s*200 900/);
    const src = face.match(/url\(["']?([^"')]+)/)![1];
    expect(new URL(src, location.href).origin).toBe(location.origin);
  }
  const ranges = faces.flatMap(face => face.match(/unicode-range:\s*([^;}]+)/i)![1].split(",").map(range => {
    const [start, end] = range.trim().replace(/^U\+/i, "").split("-").map(hex => parseInt(hex, 16));
    return [start, end ?? start];
  }));
  for (const char of text) {
    const point = char.codePointAt(0)!;
    expect(ranges.some(([start, end]) => point >= start && point <= end), char).toBe(true);
  }
  const csp = doc.querySelector('meta[http-equiv="Content-Security-Policy"]')!.getAttribute("content")!;
  expect(csp).toContain("font-src data: " + location.origin + ";");
  expect(csp).toContain("script-src 'none'");
  expect(doc.documentElement.lang).toBe("ja");
  expect(doc.body.lang).toBe("ja");
  expect(doc.querySelector("p")!.textContent).toBe(text);
  expect(doc.head.querySelectorAll("style")[1].textContent).toBe(publisher);
});

it.each([false, true])("waits for frame fonts and ignores stale readiness after unmount=%s", async (unmountEarly) => {
  const view = load("<!doctype html><html><body><p>测试正文</p></body></html>");
  // Let the native initial about:blank load finish before simulating another.
  await act(async () => { await new Promise(resolve => setTimeout(resolve, 0)); });
  props.onReady.mockClear();
  let complete!: () => void;
  const ready = new Promise<void>(resolve => { complete = resolve; });
  Object.defineProperty(view.doc, "fonts", { value: { ready }, configurable: true });
  fireEvent.load(view.frame);
  expect(props.onReady).not.toHaveBeenCalled();
  if (unmountEarly) view.unmount();
  await act(async () => { complete(); await ready; });
  expect(props.onReady).toHaveBeenCalledTimes(unmountEarly ? 0 : 1);
});

it("retains only selection endpoints until capture and clears them on deselection or reload", () => {
  const content = "<!doctype html><html><body><p>选中的正文</p></body></html>";
  const { doc, frame, rerender } = load(content);
  const ref = createRef<EpubDocumentHandle>();
  const onSelectionChange = vi.fn();
  rerender(<EpubDocument {...props} content={content} ref={ref} onSelectionChange={onSelectionChange} />);
  const clone = vi.spyOn(doc.documentElement, "cloneNode");
  try {
    const range = doc.createRange();
    range.selectNodeContents(doc.querySelector("p")!);
    doc.getSelection()!.addRange(range);
    fireEvent(doc, new Event("selectionchange"));
    expect(onSelectionChange).toHaveBeenLastCalledWith({ text: "选中的正文" });
    expect(clone).not.toHaveBeenCalled();
    expect(ref.current!.captureSelection().document).toContain("选中的正文");
    expect(clone).toHaveBeenCalledTimes(1);

    doc.getSelection()!.removeAllRanges();
    fireEvent(doc, new Event("selectionchange"));
    expect(ref.current!.captureSelection()).toEqual({ text: "" });
    expect(clone).toHaveBeenCalledTimes(1);

    doc.getSelection()!.addRange(range);
    fireEvent(doc, new Event("selectionchange"));
    fireEvent.load(frame);
    expect(ref.current!.captureSelection()).toEqual({ text: "" });
    expect(clone).toHaveBeenCalledTimes(1);
  } finally { clone.mockRestore(); }
});

function glyphRule(doc: Document) {
  const sheet = doc.head.querySelector("style")!.sheet!;
  return Array.from(sheet.cssRules).find((rule) =>
    (rule as CSSStyleRule).selectorText === ":where(img.gaiji)") as CSSStyleRule | undefined;
}

it.each(["horizontal-tb", "vertical-rl"])("provides text-sized inline gaiji defaults in %s documents", (mode) => {
  const content = `<!doctype html><html data-epub-layout="reflowable"><head><style></style></head>
    <body style="writing-mode:${mode}"><p>正文<img class="gaiji" alt="〓" width="72" height="80" src="/api/images/glyph">正文</p>
    <img class="fit" width="1200" height="1800" src="/api/images/illustration"></body></html>`;
  const { doc, rerender } = load(content);
  const rule = glyphRule(doc)!;
  expect(rule).toBeDefined();
  expect(rule.style.height).toBe("1em");
  expect(rule.style.width).toBe("auto");
  expect(rule.style.display).toBe("inline");
  expect(rule.style.getPropertyValue("vertical-align")).toBe("baseline");
  expect(doc.querySelector(".gaiji")!.getAttribute("height")).toBe("80");
  expect(doc.querySelector(".fit")!.matches(rule.selectorText)).toBe(false);
  expect(doc.documentElement.hasAttribute("data-mandara-image-only")).toBe(false);

  rerender(<EpubDocument {...props} content={content} settings={{ ...settings, fontSize: 28 }} />);
  expect(doc.head.querySelector("style")!.textContent).toContain("font-size:28px");
  expect(glyphRule(doc)!.style.height).toBe("1em");
  expect(doc.head.querySelectorAll("style")).toHaveLength(2);
});

it("places zero-specificity glyph defaults before publication styles without rewriting markup", () => {
  const publisher = ":where(img.gaiji){height:2em;vertical-align:middle}";
  const { doc } = load(`<!doctype html><html><head><style>${publisher}</style></head>
    <body><p>正文<img class="gaiji" style="height:3em" src="/api/images/glyph"></p></body></html>`);
  expect(glyphRule(doc)).toBeDefined();
  expect(doc.head.querySelectorAll("style")[1].textContent).toBe(publisher);
  expect(doc.querySelector(".gaiji")!.getAttribute("style")).toBe("height:3em");
});

it("updates frame theme defaults without sampling animated colors or changing publisher effects", () => {
  const publisher = ".narration{color:#FFFFFF;text-shadow:0 0 0.3em #FF2D95}";
  const content = `<!doctype html><html><head><style>${publisher}</style></head>
    <body><p>正文</p><p class="narration">发光文字</p></body></html>`;
  const { doc, frame, rerender } = load(content);
  const host = frame.parentElement!;
  // A color transition may still paint the previous theme during this effect.
  host.style.color = "rgb(27, 33, 48)";
  for (const [theme, foreground, scheme] of [
    ["dark", "#e6e9ef", "dark"], ["sepia", "#43382a", "light"],
  ] as const) {
    host.style.setProperty("--fg", foreground);
    host.style.colorScheme = scheme;
    rerender(<EpubDocument {...props} content={content} settings={{ ...settings, theme }} />);
    const htmlRule = doc.head.querySelector("style")!.sheet!.cssRules[0] as CSSStyleRule;
    const expected = doc.createElement("span");
    expected.style.color = foreground;
    expect(htmlRule.style.color).toBe(expected.style.color);
    expect(htmlRule.style.getPropertyValue("color-scheme")).toBe(scheme);
    expect(htmlRule.style.background).toBe("transparent");
    expect(doc.head.querySelectorAll("style")[1].textContent).toBe(publisher);
  }
});

it.each([
  '<img class="cover" width="1200" height="1800" src="/api/images/cover">',
  '<svg viewBox="0 0 1200 1800"><title>彩页</title><image width="1200" height="1800" href="/api/images/cover" /></svg>',
  '<svg viewBox="0 0 1200 1800"><svg x="100" y="100" width="400" height="600"><image href="/api/images/cover" /></svg></svg>',
])("fits standalone illustrations and their wrappers: %s", (image) => {
  const publisher = "body,.page{width:1200px;height:1800px;margin:2em;padding:1em} .cover{max-width:none;width:1200px;height:1800px}";
  const content = `<!doctype html><html data-epub-layout="reflowable"><head><style>${publisher}</style></head><body><div class="page" style="min-height:1800px"><p>${image}</p></div></body></html>`;
  const { doc, rerender } = load(content);
  expect(doc.documentElement.hasAttribute("data-mandara-image-only")).toBe(true);
  expect(doc.querySelectorAll("[data-mandara-image-wrapper]")).toHaveLength(2);
  expect(doc.querySelectorAll("[data-mandara-image]")).toHaveLength(1);
  for (const nested of doc.querySelectorAll("svg svg")) {
    expect(nested.hasAttribute("data-mandara-image")).toBe(false);
  }
  const rules = Array.from(doc.head.querySelector("style")!.sheet!.cssRules) as CSSStyleRule[];
  const rootRule = rules.find((rule) => rule.selectorText === ":root[data-mandara-image-only]")!;
  expect(rootRule.style.overflow).toBe("hidden");
  expect(rootRule.style.getPropertyValue("scrollbar-gutter")).toBe("auto");
  const imageRule = rules.find((rule) => rule.selectorText === ":root[data-mandara-image-only] [data-mandara-image]")!;
  expect(imageRule.style.getPropertyValue("max-width")).toBe("100%");
  expect(imageRule.style.getPropertyValue("max-height")).toBe("100%");
  expect(imageRule.style.getPropertyPriority("max-height")).toBe("important");
  expect(imageRule.style.getPropertyValue("object-fit")).toBe("contain");
  expect(doc.querySelector(".page")!.getAttribute("style")).toBe("min-height:1800px");
  expect(doc.head.querySelectorAll("style")[1].textContent).toBe(publisher);

  rerender(<EpubDocument {...props} content={content} settings={{ ...settings, fontSize: 28 }} />);
  expect(doc.documentElement.hasAttribute("data-mandara-image-only")).toBe(true);
  expect(doc.querySelectorAll("[data-mandara-image-wrapper]")).toHaveLength(2);
  expect(doc.head.querySelectorAll("style")).toHaveLength(2);
});

it.each([
  '<p>正文</p><img src="/api/images/illustration">',
  '<img src="/api/images/one"><img src="/api/images/two">',
  '<p><img class="gaiji" src="/api/images/glyph"></p>',
  '<table><tr><td><img src="/api/images/illustration"></td></tr></table>',
  '<p>图注</p><svg viewBox="0 0 600 800"><image href="/api/images/illustration" /></svg>',
])("keeps mixed content scrollable: %s", (body) => {
  const { doc } = load(`<!doctype html><html><body>${body}</body></html>`);
  expect(doc.documentElement.hasAttribute("data-mandara-image-only")).toBe(false);
  expect(doc.querySelectorAll("[data-mandara-image-wrapper]")).toHaveLength(0);
  expect(doc.head.querySelector("style")!.textContent).not.toContain("overflow: hidden");
});

it("preserves fixed-layout illustration scaling", () => {
  const { doc } = load(`<!doctype html><html data-epub-layout="pre-paginated"><head><meta name="viewport" content="width=600,height=800"></head><body><img width="600" height="800" src="/api/images/cover"></body></html>`);
  expect(doc.documentElement.hasAttribute("data-mandara-image-only")).toBe(false);
  expect(doc.documentElement.style.width).toBe("600px");
  expect(doc.documentElement.style.height).toBe("800px");
});

it("does not add glyph sizing defaults to fixed-layout publications", () => {
  const { doc } = load(`<!doctype html><html data-epub-layout="pre-paginated"><head>
    <meta name="viewport" content="width=600,height=800"></head><body>
    <p>正文<img class="gaiji" width="72" height="80" src="/api/images/glyph"></p></body></html>`);
  expect(glyphRule(doc)).toBeUndefined();
});

it.each([
  '<img src="/api/images/illustration" alt="插图">',
  '<svg><title>插图</title><image href="/api/images/illustration" /></svg>',
  '<svg xmlns:xlink="http://www.w3.org/1999/xlink"><title>插图</title><image xlink:href="/api/images/illustration" /></svg>',
])("opens the original illustration without replacing publication markup: %s", markup => {
  const onImageOpen = vi.fn();
  const { doc } = load('<!doctype html><html><body>' + markup + '</body></html>', onImageOpen);
  const image = doc.querySelector("img,image")!;
  const original = doc.body.innerHTML;
  expect(fireEvent.contextMenu(image)).toBe(true);
  expect(fireEvent.touchStart(image)).toBe(true);
  expect(onImageOpen).not.toHaveBeenCalled();
  fireEvent.click(image);
  expect(onImageOpen).toHaveBeenCalledWith({ src: location.origin + "/api/images/illustration", alt: "插图" });
  expect(doc.body.innerHTML).toBe(original);
});

it("preserves link navigation, modified clicks and text selection around illustrations", () => {
  const onImageOpen = vi.fn();
  const { doc } = load('<!doctype html><html><body><p>正文</p><img src="/api/images/illustration"><a href="epub:chapter/2"><img src="/api/images/linked"></a></body></html>', onImageOpen);
  const image = doc.querySelector("img")!;
  for (const modifier of ["ctrlKey", "metaKey", "altKey", "shiftKey"]) fireEvent.click(image, { [modifier]: true });
  const range = doc.createRange();
  range.selectNodeContents(doc.querySelector("p")!);
  doc.getSelection()!.addRange(range);
  fireEvent.click(image);
  doc.getSelection()!.removeAllRanges();
  fireEvent.click(doc.querySelector("a img")!);
  expect(onImageOpen).not.toHaveBeenCalled();
  expect(props.onNavigate).toHaveBeenCalledWith(2, undefined);
});
