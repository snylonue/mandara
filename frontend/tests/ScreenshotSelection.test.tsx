import { afterEach, expect, it, vi } from "vitest";
import { snapshotScreenshotDocument, screenshotRange, screenshotRoot, restoreScreenshotScroll } from "../src/screenshotSelection";

function chapter() {
  const frame = document.createElement("iframe");
  document.body.append(frame);
  return frame.contentDocument!;
}
afterEach(() => { document.querySelectorAll("iframe").forEach(node => node.remove()); vi.restoreAllMocks(); });

it("retains the complete selector context and does not mutate the reader", () => {
  const doc = chapter();
  doc.head.innerHTML = '<style id="book-style" media="(min-width: 350px)">p{color:red}</style>';
  doc.styleSheets[0].insertRule('p + p { font-size: 20.25px; font-weight: 800; }', 1);
  doc.body.innerHTML = '<article><p>前文</p><p id="selected">选中<strong>强调</strong>末尾</p><p>后文</p></article>';
  const range = doc.createRange();
  range.setStart(doc.querySelector("#selected")!.firstChild!, 1);
  range.setEnd(doc.querySelector("strong")!.firstChild!, 1);
  const original = doc.documentElement.outerHTML;
  const result = snapshotScreenshotDocument(doc, range, doc.querySelector("article")!);
  const restored = new DOMParser().parseFromString(result.document, "text/html");
  expect(doc.documentElement.outerHTML).toBe(original);
  expect(range.toString()).toBe("中强");
  expect(restored.body.innerHTML).toBe(doc.body.innerHTML);
  expect(screenshotRange(restored, result.capture).toString()).toBe("中强");
  expect(screenshotRoot(restored, result.capture).tagName).toBe("ARTICLE");
  expect(restored.querySelector("#book-style")!.getAttribute("media")).toBe("(min-width: 350px)");
  expect(restored.head.textContent).toContain("20.25px");
});

it("remaps element offsets after sanitizing unsafe siblings", () => {
  const doc = chapter();
  doc.body.innerHTML = '<p><span>前文</span><script>unsafe()</script><strong>选中</strong><iframe></iframe><span>后文</span></p>';
  const range = doc.createRange();
  range.setStart(doc.querySelector("p")!, 2);
  range.setEnd(doc.querySelector("p")!, 3);
  const result = snapshotScreenshotDocument(doc, range);
  const restored = new DOMParser().parseFromString(result.document, "text/html");
  expect(restored.querySelector("script,iframe")).toBeNull();
  expect(screenshotRange(restored, result.capture).toString()).toBe("选中");
  expect(result.capture.start.offset).toBe(1);
  expect(doc.querySelector("script")).not.toBeNull();
});

it("restores nested scroll positions and the original window scroll independently of viewport gutters", () => {
  const doc = chapter();
  doc.body.innerHTML = '<article><div id="scroller"><p>选段</p></div></article>';
  const node = doc.querySelector("#scroller")!;
  node.scrollLeft = 17; node.scrollTop = 130;
  Object.defineProperties(doc.defaultView!, { innerWidth: { value: 390 }, innerHeight: { value: 740 }, scrollX: { value: 3 }, scrollY: { value: 950 } });
  Object.defineProperty(doc.documentElement, "clientWidth", { value: 375 });
  const range = doc.createRange(); range.selectNodeContents(doc.querySelector("p")!);
  const result = snapshotScreenshotDocument(doc, range);
  expect(result.viewport).toEqual({ width: 390, height: 740 });
  const restored = chapter(); restored.documentElement.innerHTML = new DOMParser().parseFromString(result.document, "text/html").documentElement.innerHTML;
  const scroll = vi.spyOn(restored.defaultView!, "scrollTo").mockImplementation(() => {});
  restoreScreenshotScroll(restored, result.capture);
  expect([restored.querySelector("#scroller")!.scrollLeft, restored.querySelector("#scroller")!.scrollTop]).toEqual([17, 130]);
  expect(scroll).toHaveBeenCalledWith(3, 950);
});

it("preserves disabled and adopted stylesheet state", () => {
  const doc = chapter();
  doc.head.innerHTML = '<style id="disabled">p{color:red}</style>';
  Object.defineProperty(doc.styleSheets[0], "disabled", { value: true });
  const adopted = new CSSStyleSheet(); adopted.insertRule('p { font-size: 20.25px; }');
  Object.defineProperty(doc, "adoptedStyleSheets", { value: [adopted] });
  doc.body.innerHTML = "<p>选段</p>";
  const range = doc.createRange(); range.selectNodeContents(doc.body);
  const result = snapshotScreenshotDocument(doc, range);
  const restored = new DOMParser().parseFromString(result.document, "text/html");
  expect(restored.querySelector("#disabled")!.getAttribute("media")).toBe("not all");
  expect(restored.head.textContent).toContain("20.25px");
});

it("rejects stale or collapsed endpoint descriptors instead of capturing a different passage", () => {
  const doc = chapter(); doc.body.innerHTML = "<p>选段</p>";
  const range = doc.createRange(); range.selectNodeContents(doc.body);
  const { capture } = snapshotScreenshotDocument(doc, range);
  expect(() => screenshotRange(doc, { ...capture, start: { path: [999], offset: 0 } })).toThrow("selection-context");
  expect(() => screenshotRange(doc, { ...capture, end: capture.start })).toThrow("selection-context");
});
