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

it("does not add glyph sizing defaults to fixed-layout publications", () => {
  const { doc } = load(`<!doctype html><html data-epub-layout="pre-paginated"><head>
    <meta name="viewport" content="width=600,height=800"></head><body>
    <p>正文<img class="gaiji" width="72" height="80" src="/api/images/glyph"></p></body></html>`);
  expect(glyphRule(doc)).toBeUndefined();
});
