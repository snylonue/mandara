import { toCanvas } from "html-to-image";

function fontFaces(doc: Document): string {
  function collect(rules: CSSRuleList): string[] {
    return Array.from(rules).flatMap(rule => {
      if (rule.type === CSSRule.FONT_FACE_RULE) return [rule.cssText];
      return "cssRules" in rule ? collect((rule as CSSGroupingRule).cssRules) : [];
    });
  }
  // EPUB fonts are already data URLs. Explicit CSS also avoids cross-realm font
  // discovery losing descendant fonts in an iframe.
  return Array.from(doc.styleSheets).flatMap(sheet => collect(sheet.cssRules)).join("\n");
}

/** Keep publisher CSS isolated, including while measuring and rasterizing an excerpt. */
export async function renderEpubScreenshot(source: string, width: number, maxHeight: number): Promise<HTMLCanvasElement> {
  const frame = document.createElement("iframe");
  frame.setAttribute("sandbox", "allow-same-origin");
  frame.setAttribute("aria-hidden", "true");
  frame.tabIndex = -1;
  frame.style.cssText = "position:fixed;left:-100000px;top:0;border:0;pointer-events:none;width:" + width + "px;height:600px";
  const csp = "default-src 'none'; script-src 'none'; style-src 'unsafe-inline'; img-src data: " + location.origin + "/api/images/; font-src data:; base-uri 'none'; form-action 'none'";
  // Install CSP before any publication markup, as in the reader frame.
  frame.srcdoc = source.replace(/<!doctype html>/i, '<!doctype html><meta http-equiv="Content-Security-Policy" content="' + csp + '">');
  try {
    await new Promise<void>((resolve, reject) => {
      const timer = window.setTimeout(() => reject(new Error("epub-load-timeout")), 15000);
      frame.onload = () => { clearTimeout(timer); resolve(); };
      frame.onerror = () => { clearTimeout(timer); reject(new Error("epub-load-failed")); };
      document.body.append(frame);
    });
    const doc = frame.contentDocument!;
    await doc.fonts?.ready;
    await Promise.all(Array.from(doc.images, image => image.decode()));
    const root = doc.documentElement;
    const vertical = getComputedStyle(doc.body).writingMode.startsWith("vertical");
    // Let the excerpt grow on its block axis, not inside the reader's scroll viewport.
    root.style.setProperty("transform", "none", "important");
    root.style.setProperty("overflow", "visible", "important");
    root.style.setProperty("width", width + "px", "important");
    root.style.setProperty("height", vertical ? "600px" : "auto", "important");
    const bounds = [root, ...doc.body.querySelectorAll("*")].map(node => node.getBoundingClientRect());
    const left = Math.min(0, ...bounds.map(rect => rect.left));
    const top = Math.min(0, ...bounds.map(rect => rect.top));
    // Vertical-rl content can overflow to the left, including inside horizontal documents.
    const captureWidth = Math.ceil(Math.max(width, root.scrollWidth, ...bounds.map(rect => rect.right)) - left);
    // scrollHeight is at least the iframe viewport height even for a short passage.
    const captureHeight = Math.ceil(Math.max(1, doc.body.scrollHeight, root.scrollHeight > 600 ? root.scrollHeight : 0, ...bounds.map(rect => rect.bottom)) - top);
    if (captureWidth > maxHeight || captureHeight > maxHeight || captureWidth * captureHeight > width * maxHeight) throw new Error("selection-size");
    return await toCanvas(root, {
      width: captureWidth, height: captureHeight, pixelRatio: 1,
      // Preserve the measured layout while expanding the SVG viewport to include overflow.
      style: { width: root.getBoundingClientRect().width + "px", height: root.getBoundingClientRect().height + "px", transform: "translate(" + -left + "px," + -top + "px)" },
      fontEmbedCSS: fontFaces(doc),
      // The computed clone has all presentation; source rules need not be rendered again.
      filter: node => !["HEAD", "STYLE", "SCRIPT"].includes(node.nodeName),
    });
  } finally {
    frame.onload = null;
    frame.onerror = null;
    frame.remove();
  }
}
