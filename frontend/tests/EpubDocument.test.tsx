import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { fireEvent, render } from "@testing-library/react";
import { EpubDocument } from "../src/components/EpubDocument";
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

function load(content: string) {
  const view = render(<EpubDocument {...props} content={content} />);
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
