import { rasterizeScreenshot, waitForScreenshotImage } from "./screenshotImage";
import { screenshotPixelRatio } from "./screenshotCanvas";
import { screenshotStep, screenshotTask, waitForScreenshot } from "./screenshotTask";

function fontFamilies(value: string): string[] {
  return value.split(",").map(family => family.trim().replace(/^['"]|['"]$/g, "").toLowerCase());
}

function includesText(range: string, text: string): boolean {
  if (!range) return true;
  const ranges = range.split(",").map(part => {
    const [start, end] = part.trim().replace(/^U\+/i, "").split("-");
    return [parseInt(start.replace(/\?/g, "0"), 16), parseInt((end || start).replace(/\?/g, "f"), 16)];
  });
  return Array.from(text).some(char => ranges.some(([start, end]) => char.codePointAt(0)! >= start && char.codePointAt(0)! <= end));
}

async function fontFaces(doc: Document, signal: AbortSignal): Promise<string> {
  const used = new Set([doc.body, ...doc.body.querySelectorAll("*")].flatMap(node => fontFamilies(getComputedStyle(node).fontFamily)));
  const text = doc.body.textContent || "";
  function collect(rules: CSSRuleList): string[] {
    return Array.from(rules).flatMap(rule => {
      if (rule.type === CSSRule.FONT_FACE_RULE) {
        const style = (rule as CSSFontFaceRule).style;
        return fontFamilies(style.getPropertyValue("font-family")).some(family => used.has(family))
          && includesText(style.getPropertyValue("unicode-range"), text) ? [rule.cssText] : [];
      }
      return "cssRules" in rule ? collect((rule as CSSGroupingRule).cssRules) : [];
    });
  }
  // Explicit CSS avoids cross-realm discovery losing descendant fonts in an iframe.
  // Embed self-hosted reader fonts as well as the publication's existing data URLs.
  let css = Array.from(doc.styleSheets).flatMap(sheet => collect(sheet.cssRules)).join("\n");
  const resources = new Map<string, Promise<string>>();
  const urls = Array.from(css.matchAll(/url\(\s*['"]?([^'"\)]+)['"]?\s*\)/g));
  await Promise.all(urls.map(async ([, value]) => {
    if (value.startsWith("data:")) return;
    const url = new URL(value.trim(), location.href);
    if (url.origin !== location.origin) throw new Error("font-origin");
    let embedded = resources.get(url.href);
    if (!embedded) {
      embedded = (async () => {
        const response = await fetch(url.href, { signal });
        if (!response.ok) throw new Error("font-load");
        const blob = await response.blob();
        return new Promise<string>((resolve, reject) => {
          const reader = new FileReader();
          reader.onload = () => resolve(reader.result as string);
          reader.onerror = () => reject(reader.error);
          reader.readAsDataURL(blob);
        });
      })();
      resources.set(url.href, embedded);
    }
    await waitForScreenshot(embedded, signal);
  }));
  for (const [reference, value] of urls) {
    if (!value.startsWith("data:")) css = css.split(reference).join('url("' + await resources.get(new URL(value.trim(), location.href).href)! + '")');
  }
  return css;
}

/** Keep publisher CSS isolated, including while measuring and rasterizing an excerpt. */
export function renderEpubScreenshot(source: string, width: number, maxHeight: number, viewportHeight = 600, parent?: AbortSignal): Promise<HTMLCanvasElement> {
  return screenshotTask(async signal => {
    const frame = document.createElement("iframe");
    frame.setAttribute("sandbox", "allow-same-origin");
    frame.setAttribute("aria-hidden", "true");
    frame.tabIndex = -1;
    frame.style.cssText = "position:fixed;left:-100000px;top:0;border:0;pointer-events:none;width:" + width + "px;height:" + viewportHeight + "px";
    const csp = "default-src 'none'; script-src 'none'; style-src 'unsafe-inline'; img-src data: " + location.origin + "/api/images/; font-src data: " + location.origin + "; base-uri 'none'; form-action 'none'";
    // Install CSP before any publication markup, as in the reader frame.
    frame.srcdoc = source.replace(/<!doctype html>/i, '<!doctype html><meta http-equiv="Content-Security-Policy" content="' + csp + '">');
    try {
      await screenshotStep("loadDocument", () => new Promise<void>((resolve, reject) => {
        frame.onload = () => resolve();
        frame.onerror = () => reject(new Error("epub-load-failed"));
        document.body.append(frame);
      }), signal);
      const doc = frame.contentDocument!;
      await screenshotStep("loadFonts", () => doc.fonts?.ready, signal);
      await screenshotStep("loadImages", () => Promise.all(Array.from(doc.images, image => {
        image.loading = "eager";
        return waitForScreenshotImage(image, signal);
      })), signal);
      const root = doc.documentElement;
      // HTML accepts unbound epub: attributes, but the SVG image must be valid XML.
      // Declare the prefix instead of dropping semantics or changing CSS selectors.
      root.setAttribute("xmlns:epub", "http://www.idpf.org/2007/ops");
      const vertical = getComputedStyle(doc.body).writingMode.startsWith("vertical");
      // Let the excerpt grow on its block axis, not inside the reader's scroll viewport.
      root.style.setProperty("transform", "none", "important");
      root.style.setProperty("overflow", "visible", "important");
      root.style.setProperty("width", width + "px", "important");
      root.style.setProperty("height", vertical ? viewportHeight + "px" : "auto", "important");
      const bounds = [root, ...doc.body.querySelectorAll("*")].map(node => node.getBoundingClientRect());
      const left = Math.min(0, ...bounds.map(rect => rect.left));
      const top = Math.min(0, ...bounds.map(rect => rect.top));
      // Vertical-rl content can overflow to the left, including inside horizontal documents.
      const captureWidth = Math.ceil(Math.max(width, root.scrollWidth, ...bounds.map(rect => rect.right)) - left);
      // scrollHeight is at least the iframe viewport height even for a short passage.
      const captureHeight = Math.ceil(Math.max(1, doc.body.scrollHeight, root.scrollHeight > viewportHeight ? root.scrollHeight : 0, ...bounds.map(rect => rect.bottom)) - top);
      if (captureWidth > maxHeight || captureHeight > maxHeight || captureWidth * captureHeight > width * maxHeight) throw new Error("selection-size");
      const fontEmbedCSS = await screenshotStep("embedFonts", () => fontFaces(doc, signal), signal);
      return await screenshotStep("rasterize", () => rasterizeScreenshot(root, {
        signal,
        width: captureWidth, height: captureHeight, pixelRatio: screenshotPixelRatio(captureWidth, captureHeight),
        // Preserve the measured layout while expanding the SVG viewport to include overflow.
        style: { width: root.getBoundingClientRect().width + "px", height: root.getBoundingClientRect().height + "px", transform: "translate(" + -left + "px," + -top + "px)" },
        fontEmbedCSS,
        // The computed clone has all presentation; source rules need not be rendered again.
        filter: node => !["HEAD", "STYLE", "SCRIPT"].includes(node.nodeName),
      }), signal);
    } finally {
      frame.onload = null;
      frame.onerror = null;
      frame.remove();
    }
  }, parent);
}
