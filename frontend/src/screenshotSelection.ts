export interface ScreenshotPoint {
  path: number[];
  offset: number;
}

export interface ScreenshotCapture {
  root: number[];
  start: ScreenshotPoint;
  end: ScreenshotPoint;
  scroll: { path: number[]; x: number; y: number }[];
  windowScroll: { x: number; y: number };
}

function pathTo(root: Node, node: Node): number[] {
  const path: number[] = [];
  while (node !== root) {
    const parent = node.parentNode;
    if (!parent) throw new Error("selection-context");
    path.unshift(Array.prototype.indexOf.call(parent.childNodes, node));
    node = parent;
  }
  return path;
}

function nodeAt(root: Node, path: number[]): Node {
  for (const index of path) {
    const child = root.childNodes[index];
    if (!child) throw new Error("selection-context");
    root = child;
  }
  return root;
}

/** Restore endpoints only after the full, sanitized publication has been mounted. */
export function screenshotRange(doc: Document, capture: ScreenshotCapture): Range {
  const range = doc.createRange();
  range.setStart(nodeAt(doc.documentElement, capture.start.path), capture.start.offset);
  range.setEnd(nodeAt(doc.documentElement, capture.end.path), capture.end.offset);
  if (range.collapsed) throw new Error("selection-context");
  return range;
}

export function screenshotRoot(doc: Document, capture: ScreenshotCapture): Element {
  return nodeAt(doc.documentElement, capture.root) as Element;
}

export function restoreScreenshotScroll(doc: Document, capture: ScreenshotCapture): void {
  for (const scroll of capture.scroll) {
    const node = nodeAt(doc.documentElement, scroll.path) as Element;
    node.scrollLeft = scroll.x;
    node.scrollTop = scroll.y;
  }
  doc.defaultView?.scrollTo(capture.windowScroll.x, capture.windowScroll.y);
}

/** Preserve selectors, counters, viewport units and unselected text until after layout. */
export function snapshotScreenshotDocument(doc: Document, selected: Range, contentRoot: Element = doc.documentElement) {
  const snapshot = doc.implementation.createHTMLDocument("");
  const html = doc.documentElement.cloneNode(true) as HTMLElement;
  snapshot.replaceChild(html, snapshot.documentElement);
  const cloneOf = (node: Node) => nodeAt(html, pathTo(doc.documentElement, node));
  const clonedRoot = cloneOf(contentRoot);
  // A live range adjusts its offsets when unsafe nodes are removed below.
  const range = snapshot.createRange();
  range.setStart(cloneOf(selected.startContainer), selected.startOffset);
  range.setEnd(cloneOf(selected.endContainer), selected.endOffset);
  const scroll = Array.from(doc.querySelectorAll("*")).filter(node => node.scrollLeft || node.scrollTop)
    .map(node => ({ node: cloneOf(node), x: node.scrollLeft, y: node.scrollTop }));
  // Copy CSSOM rules, including application stylesheets and edits not reflected in textContent.
  for (const sheet of doc.styleSheets) {
    const css = Array.from(sheet.cssRules, rule => rule.cssText).join("\n");
    const owner = sheet.ownerNode;
    if (owner && doc.documentElement.contains(owner)) {
      const original = cloneOf(owner);
      const style = snapshot.createElement("style");
      for (const attribute of Array.from((owner as Element).attributes || [])) style.setAttribute(attribute.name, attribute.value);
      if (sheet.disabled) style.media = "not all";
      style.textContent = css;
      original.parentNode!.replaceChild(style, original);
    }
  }
  for (const sheet of doc.adoptedStyleSheets || []) {
    const style = snapshot.createElement("style");
    style.textContent = Array.from(sheet.cssRules, rule => rule.cssText).join("\n");
    if (sheet.disabled) style.media = "not all";
    snapshot.head.append(style);
  }
  for (const node of snapshot.querySelectorAll("script,iframe,object,embed,link,meta,base,form")) node.remove();
  for (const node of snapshot.querySelectorAll("*")) {
    for (const attribute of Array.from(node.attributes)) {
      if (/^on/i.test(attribute.name) || ["srcdoc", "autofocus"].includes(attribute.name)) node.removeAttribute(attribute.name);
    }
  }
  const capture: ScreenshotCapture = {
    root: pathTo(html, clonedRoot),
    start: { path: pathTo(html, range.startContainer), offset: range.startOffset },
    end: { path: pathTo(html, range.endContainer), offset: range.endOffset },
    scroll: scroll.filter(item => html.contains(item.node)).map(item => ({ path: pathTo(html, item.node), x: item.x, y: item.y })),
    windowScroll: { x: doc.defaultView?.scrollX || 0, y: doc.defaultView?.scrollY || 0 },
  };
  return {
    document: "<!doctype html>" + html.outerHTML,
    capture,
    viewport: {
      width: doc.defaultView?.innerWidth || doc.documentElement.clientWidth || 772,
      height: doc.defaultView?.innerHeight || doc.documentElement.clientHeight || 600,
    },
  };
}
