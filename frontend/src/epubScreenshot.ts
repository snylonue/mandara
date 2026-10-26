import { prepareScreenshotImages, rasterizeScreenshot, waitForScreenshotImage } from "./screenshotImage";
import { screenshotStep, screenshotTask, waitForScreenshot } from "./screenshotTask";
import { screenshotRange, screenshotRoot, restoreScreenshotScroll, type ScreenshotCapture } from "./screenshotSelection";

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

async function fontFaces(doc: Document, signal: AbortSignal, elements: Element[], text: string): Promise<string> {
  const used = new Set(elements.flatMap(node => fontFamilies(getComputedStyle(node).fontFamily)));
  for (const node of elements) for (const pseudo of ["::before", "::after", "::marker", "::first-letter"]) {
    const style = getComputedStyle(node, pseudo);
    fontFamilies(style.fontFamily).forEach(family => used.add(family));
    if (style.content && !["none", "normal"].includes(style.content)) text += style.content + "0123456789";
  }
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
export function renderEpubScreenshot(source: string, width: number, maxHeight: number, viewportHeight = 600, parent?: AbortSignal, capture?: ScreenshotCapture): Promise<HTMLCanvasElement> {
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
      const range = capture ? screenshotRange(doc, capture) : null;
      let selected = range?.commonAncestorContainer || doc.body;
      if (selected.nodeType !== Node.ELEMENT_NODE) selected = selected.parentNode!;
      let whole: Element | null = null;
      if (range && range.startContainer === range.endContainer && range.startContainer.nodeType === Node.ELEMENT_NODE) {
        const container = range.startContainer as Element;
        if (range.startOffset === 0 && range.endOffset === container.childNodes.length) whole = container;
        else if (range.endOffset === range.startOffset + 1 && container.childNodes[range.startOffset]?.nodeType === Node.ELEMENT_NODE) whole = container.childNodes[range.startOffset] as Element;
      }
      const target = whole || selected as Element;
      const elements = [target, ...target.querySelectorAll("*")].filter(node => !range || range.intersectsNode(node));
      const fontEmbedCSS = await screenshotStep("embedFonts", () => fontFaces(doc, signal, elements, range?.toString() || doc.body.textContent || ""), signal);
      const fonts = doc.createElement("style");
      fonts.textContent = fontEmbedCSS;
      doc.head.append(fonts);
      await screenshotStep("loadFonts", async () => {
        await doc.fonts?.ready;
        const requests = new Map<string, Promise<FontFace[]> | undefined>();
        const sample = (range?.toString() || doc.body.textContent || "") + "0123456789";
        for (const node of elements) for (const pseudo of [null, "::before", "::after", "::marker", "::first-letter"]) {
          const style = getComputedStyle(node, pseudo);
          if (!style.fontFamily) continue;
          const font = style.font || [style.fontStyle, style.fontWeight, style.fontSize, style.fontFamily].filter(Boolean).join(" ");
          const text = sample + (pseudo ? style.content || "" : "");
          const key = font + "\0" + text;
          if (!requests.has(key)) requests.set(key, doc.fonts?.load(font, text));
        }
        await Promise.all(requests.values());
      }, signal);
      const resourceElements = [...elements];
      const scope = capture ? screenshotRoot(doc, capture) : doc.body;
      for (let ancestor = target.parentElement; ancestor && scope.contains(ancestor); ancestor = ancestor.parentElement) resourceElements.push(ancestor);
      const images = await screenshotStep("loadImages", async () => {
        await Promise.all([
        ...elements.filter(node => node.localName === "img").map(node => {
          const image = node as HTMLImageElement;
          const loading = image.getAttribute("loading");
          image.loading = "eager";
          return waitForScreenshotImage(image, signal).finally(() => {
            if (loading === null) image.removeAttribute("loading");
            else image.setAttribute("loading", loading);
          });
        }),
        ]);
        return prepareScreenshotImages(resourceElements, signal);
      }, signal);
      const root = doc.documentElement;
      root.setAttribute("xmlns:epub", "http://www.idpf.org/2007/ops");
      if (capture) restoreScreenshotScroll(doc, capture);
      else {
        // Legacy callers without a selection descriptor export the complete supplied document.
        const vertical = getComputedStyle(doc.body).writingMode.startsWith("vertical");
        root.style.setProperty("transform", "none", "important");
        root.style.setProperty("overflow", "visible", "important");
        root.style.setProperty("width", width + "px", "important");
        root.style.setProperty("height", vertical ? viewportHeight + "px" : "auto", "important");
      }
      const bounds = range
        ? [whole ? whole.getBoundingClientRect() : range.getBoundingClientRect()]
        : [root, ...doc.body.querySelectorAll("*")].map(node => node.getBoundingClientRect());
      const left = Math.floor(Math.min(...bounds.map(rect => rect.left), ...(range ? [] : [0])));
      const top = Math.floor(Math.min(...bounds.map(rect => rect.top), ...(range ? [] : [0])));
      const right = Math.ceil(Math.max(...bounds.map(rect => rect.right), ...(range ? [] : [width, root.scrollWidth])));
      const bottom = Math.ceil(Math.max(...bounds.map(rect => rect.bottom), ...(range ? [] : [1, doc.body.scrollHeight, root.scrollHeight > viewportHeight ? root.scrollHeight : 0])));
      const captureWidth = right - left;
      const captureHeight = bottom - top;
      if (!(captureWidth > 0 && captureHeight > 0) || captureWidth > maxHeight || captureHeight > maxHeight || captureWidth * captureHeight > width * maxHeight) throw new Error("selection-size");
      const mask = range && !whole ? Array.from(range.getClientRects(), rect => ({ x: rect.left - left, y: rect.top - top, width: rect.width, height: rect.height })) : undefined;
      // Keep the publication paint tree, including every ancestor effect. The TXT
      // application shell is outside this boundary; crop only after presentation freezes.
      const paintRoot = range ? scope : root;
      const backdrop: string[] = [];
      for (let ancestor = paintRoot.parentElement; ancestor; ancestor = ancestor.parentElement) backdrop.unshift(getComputedStyle(ancestor).backgroundColor);
      return await screenshotStep("rasterize", () => rasterizeScreenshot(paintRoot as HTMLElement, {
        signal,
        width: captureWidth, height: captureHeight,
        clip: { x: left + (doc.defaultView?.scrollX || 0), y: top + (doc.defaultView?.scrollY || 0), width: captureWidth, height: captureHeight },
        mask,
        backdrop,
        images,
        fontEmbedCSS,
      }), signal);
    } finally {
      frame.onload = null;
      frame.onerror = null;
      frame.remove();
    }
  }, parent);
}
