import { snapdom } from "@zumer/snapdom";
import { screenshotPixelRatio } from "./screenshotCanvas";
import { waitForScreenshot } from "./screenshotTask";

/** Image load is sufficient for drawing; decode() can stall even after SVG load. */
export function waitForScreenshotImage(image: HTMLImageElement, signal: AbortSignal): Promise<void> {
  return new Promise((resolve, reject) => {
    const cleanup = () => {
      image.removeEventListener("load", loaded);
      image.removeEventListener("error", failed);
      signal.removeEventListener("abort", aborted);
    };
    const loaded = () => { cleanup(); resolve(); };
    const failed = () => { cleanup(); reject(new Error("image-load")); };
    const aborted = () => { cleanup(); reject(signal.reason); };
    image.addEventListener("load", loaded);
    image.addEventListener("error", failed);
    signal.addEventListener("abort", aborted, { once: true });
    if (signal.aborted) aborted();
    else if (image.complete) {
      if (image.naturalWidth > 0) loaded();
      else failed();
    }
  });
}

/** Resolve resources under the same origin policy as the isolated reader frame. */
async function imageData(reference: string, signal: AbortSignal, resources: Map<string, Promise<string>>): Promise<string> {
  signal.throwIfAborted();
  const url = new URL(reference, location.href);
  // Match the export frame's image CSP instead of fetching arbitrary book URLs.
  if (url.origin !== location.origin || !url.pathname.startsWith("/api/images/")) throw new Error("image-origin");
  const fragment = url.hash;
  url.hash = "";
  let data = resources.get(url.href);
  if (!data) {
    data = (async () => {
      const response = await fetch(url.href, { signal });
      if (!response.ok) throw new Error("image-load");
      const blob = await response.blob();
      return new Promise<string>((resolve, reject) => {
        const reader = new FileReader();
        const aborted = () => reader.abort();
        reader.onload = () => resolve(reader.result as string);
        reader.onerror = () => reject(reader.error);
        reader.onabort = () => reject(signal.reason);
        reader.onloadend = () => signal.removeEventListener("abort", aborted);
        if (signal.aborted) { reject(signal.reason); return; }
        signal.addEventListener("abort", aborted, { once: true });
        try { reader.readAsDataURL(blob); }
        catch (error) { signal.removeEventListener("abort", aborted); reject(error); }
      });
    })();
    resources.set(url.href, data);
  }
  return await waitForScreenshot(data, signal) + fragment;
}

function imageURLs(css: string, replace: (url: string) => string): string {
  return css.replace(/(\/\*[^]*?\*\/|"(?:\\.|[^"\\])*"|'(?:\\.|[^'\\])*')|url\(\s*(?:"((?:\\.|[^"\\])*)"|'((?:\\.|[^'\\])*)'|([^)]*))\s*\)/gi, (match, literal, quoted, single, bare) => {
    if (literal) return match;
    const reference = (quoted ?? single ?? bare).trim();
    const embedded = replace(reference);
    return embedded === reference ? match : 'url("' + embedded + '")';
  });
}

/** Preflight resources explicitly: SnapDOM silently drops some missing CSS images. */
export async function prepareScreenshotImages(elements: Element[], signal: AbortSignal): Promise<Map<string, string>> {
  const references = new Set<string>(), cssImages = new Set<string>();
  const add = (reference: string | null) => {
    if (reference && !reference.startsWith("data:") && !reference.startsWith("#") && !reference.startsWith("about:srcdoc#")) references.add(reference);
  };
  for (const node of elements) {
    if (node.localName === "img") add((node as HTMLImageElement).currentSrc || node.getAttribute("src"));
    if (node.localName === "image") add(node.getAttribute("href") ?? node.getAttribute("xlink:href"));
    for (const pseudo of [null, "::before", "::after", "::marker"]) {
      const style = getComputedStyle(node, pseudo);
      for (const property of ["background-image", "mask-image", "border-image-source", "list-style-image", "shape-outside", "content"]) {
        imageURLs(style.getPropertyValue(property), reference => { add(reference); if (references.has(reference)) cssImages.add(reference); return reference; });
      }
    }
  }
  const resources = new Map<string, Promise<string>>();
  const embedded = new Map<string, string>();
  await Promise.all(Array.from(references, async reference => {
    const data = await imageData(reference, signal, resources);
    if (cssImages.has(reference)) {
      const image = new Image(); image.src = data;
      try { await waitForScreenshotImage(image, signal); }
      finally { image.removeAttribute("src"); }
    }
    embedded.set(reference, data);
  }));
  return embedded;
}

/** Prepare SVG references without detached load events; optionally defer attribute rewrites. */
export async function embedScreenshotSvgImages(root: ParentNode, signal: AbortSignal, references?: Map<string, string>): Promise<void> {
  const xlink = "http://www.w3.org/1999/xlink";
  const resources = new Map<string, Promise<string>>();
  await Promise.all(Array.from(root.querySelectorAll<SVGImageElement>("svg image"), async image => {
    const reference = image.getAttribute("href") ?? image.getAttributeNS(xlink, "href") ?? image.getAttribute("xlink:href");
    if (!reference || reference.startsWith("data:")) return;
    const embedded = await imageData(reference, signal, resources);
    signal.throwIfAborted();
    if (references) { references.set(reference, embedded); return; }
    if (image.hasAttribute("href")) image.setAttribute("href", embedded);
    if (image.hasAttributeNS(xlink, "href") || image.hasAttribute("xlink:href")) {
      image.removeAttributeNS(xlink, "href"); image.removeAttribute("xlink:href");
      image.setAttributeNS(xlink, "xlink:href", embedded);
    }
  }));
}

/** Allow SVG font painting a frame without depending on foreground-only rAF. */
function paintReady(signal: AbortSignal): Promise<void> {
  return new Promise((resolve, reject) => {
    const cleanup = () => {
      cancelAnimationFrame(frame);
      clearTimeout(timer);
      signal.removeEventListener("abort", aborted);
    };
    const ready = () => { cleanup(); resolve(); };
    const aborted = () => { cleanup(); reject(signal.reason); };
    const frame = requestAnimationFrame(ready);
    const timer = window.setTimeout(ready, 50);
    signal.addEventListener("abort", aborted, { once: true });
    if (signal.aborted) aborted();
  });
}

export interface ScreenshotRect { x: number; y: number; width: number; height: number }

/** Freeze presentation with SnapDOM; own decoding, cancellation and allocation limits. */
export async function rasterizeScreenshot(node: HTMLElement, options: {
  width: number;
  height: number;
  signal: AbortSignal;
  fontEmbedCSS?: string;
  clip?: ScreenshotRect;
  mask?: ScreenshotRect[];
  backdrop?: string[];
  images?: Map<string, string>;
}): Promise<HTMLCanvasElement> {
  const { signal } = options;
  signal.throwIfAborted();
  const result = await waitForScreenshot(snapdom(node, {
    dpr: 1,
    scale: 1,
    embedFonts: false,
    clip: options.clip,
    // Computed presentation is frozen; replaying publisher CSS restyles synthetic pseudo nodes.
    filter: element => !["HEAD", "STYLE", "SCRIPT"].includes(element.nodeName),
    filterMode: "remove",
    plugins: [{
      name: "reader-screenshot",
      beforeClone() { signal.throwIfAborted(); },
      afterClone(context) {
        signal.throwIfAborted();
        // Rewrite resources only after computed styles and attribute selectors have been frozen.
        const replace = (url: string) => options.images?.get(url) || url;
        context.classCSS = imageURLs(context.classCSS || "", replace);
        for (const image of [context.clone, ...context.clone?.querySelectorAll("*") || []]) {
          if (!image) continue;
          const source = (context.nodeMap as Map<Element, HTMLImageElement> | undefined)?.get(image);
          const chosen = source?.localName === "img" && options.images?.get(source.currentSrc);
          if (chosen) { image.setAttribute("src", chosen); image.removeAttribute("srcset"); }
          if (image.hasAttribute("style")) image.setAttribute("style", imageURLs(image.getAttribute("style")!, replace));
          for (const attribute of ["src", "href", "xlink:href"]) {
            const reference = image.getAttribute(attribute);
            const embedded = reference && options.images?.get(reference);
            if (embedded) {
              image.removeAttribute(attribute);
              if (attribute === "xlink:href") image.setAttributeNS("http://www.w3.org/1999/xlink", attribute, embedded);
              else image.setAttribute(attribute, embedded);
              if (attribute === "src") image.removeAttribute("srcset");
            }
          }
        }
      },
      beforeRender(context) {
        signal.throwIfAborted();
        context.fontsCSS = options.fontEmbedCSS || "";
        context.clone?.setAttribute("xmlns:epub", "http://www.idpf.org/2007/ops");
        // SVG images use a different scrollbar environment. Do not invent reserved gutters
        // when the source uses overlay/hidden scrollbars (including headless Chromium).
        const nodes = context.nodeMap as Map<HTMLElement, HTMLElement> | undefined;
        if (nodes) for (const [clone, source] of nodes) {
          if (source.namespaceURI !== "http://www.w3.org/1999/xhtml") continue;
          const style = source.ownerDocument.defaultView!.getComputedStyle(source);
          if (![style.overflowX, style.overflowY, style.overflow].some(value => value === "auto" || value === "scroll")) continue;
          const borderX = (parseFloat(style.borderLeftWidth) || 0) + (parseFloat(style.borderRightWidth) || 0);
          const borderY = (parseFloat(style.borderTopWidth) || 0) + (parseFloat(style.borderBottomWidth) || 0);
          if (source.offsetWidth - source.clientWidth - borderX < 1 && source.offsetHeight - source.clientHeight - borderY < 1) {
            clone.style.setProperty("scrollbar-width", "none", "important");
            clone.setAttribute("data-reader-overlay-scrollbar", "");
          }
        }
        context.classCSS = (context.classCSS || "") + '[data-reader-overlay-scrollbar]::-webkit-scrollbar{display:none!important;width:0!important;height:0!important}';
      },
    }],
  }), signal);
  signal.throwIfAborted();
  const failures = result.warnings.filter(warning => warning.code !== "reconcile-risk");
  if (failures.length) throw new Error("capture-" + failures.map(warning => warning.code).join(","));
  const image = new Image();
  image.crossOrigin = "anonymous";
  try {
    image.src = result.url;
    await waitForScreenshotImage(image, signal);
    await paintReady(signal);
    signal.throwIfAborted();
    const canvas = document.createElement("canvas");
    const ratio = screenshotPixelRatio(options.width, options.height);
    canvas.width = Math.floor(options.width * ratio);
    canvas.height = Math.floor(options.height * ratio);
    canvas.style.width = options.width + "px";
    canvas.style.height = options.height + "px";
    const ctx = canvas.getContext("2d");
    if (!ctx) throw new Error("canvas-unavailable");
    // Clip pixels, never split/reflow source text or alter selector/counter context.
    if (options.mask) {
      ctx.beginPath();
      for (const rect of options.mask) ctx.rect(rect.x * ratio, rect.y * ratio, rect.width * ratio, rect.height * ratio);
      ctx.clip();
    }
    for (const color of options.backdrop || []) {
      ctx.fillStyle = color;
      ctx.fillRect(0, 0, canvas.width, canvas.height);
    }
    ctx.drawImage(image, 0, 0, canvas.width, canvas.height);
    return canvas;
  } finally {
    image.removeAttribute("src");
  }
}
