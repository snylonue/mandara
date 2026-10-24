import { toSvg } from "html-to-image";
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

/** Embed SVG image references while their owner document is still connected.
 * The clone library otherwise waits for load on detached SVGImageElements.
 */
export async function embedScreenshotSvgImages(root: ParentNode, signal: AbortSignal): Promise<void> {
  const xlink = "http://www.w3.org/1999/xlink";
  const resources = new Map<string, Promise<string>>();
  await Promise.all(Array.from(root.querySelectorAll<SVGImageElement>("svg image"), async image => {
    const reference = image.getAttribute("href") ?? image.getAttributeNS(xlink, "href") ?? image.getAttribute("xlink:href");
    if (!reference || reference.startsWith("data:")) return;
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
    const embedded = await waitForScreenshot(data, signal) + fragment;
    signal.throwIfAborted();
    if (image.hasAttribute("href")) image.setAttribute("href", embedded);
    if (image.hasAttributeNS(xlink, "href") || image.hasAttribute("xlink:href")) {
      image.removeAttributeNS(xlink, "href");
      image.removeAttribute("xlink:href");
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

/** Keep the styled SVG clone, but own the image lifetime instead of toCanvas's decode wait. */
export async function rasterizeScreenshot(node: HTMLElement, options: NonNullable<Parameters<typeof toSvg>[1]> & { width: number; height: number; signal: AbortSignal }): Promise<HTMLCanvasElement> {
  const { signal, ...renderOptions } = options;
  signal.throwIfAborted();
  const svg = await waitForScreenshot(toSvg(node, {
    ...renderOptions,
    fetchRequestInit: { ...renderOptions.fetchRequestInit, signal },
  }), signal);
  signal.throwIfAborted();
  const image = new Image();
  image.crossOrigin = "anonymous";
  try {
    image.src = svg;
    await waitForScreenshotImage(image, signal);
    await paintReady(signal);
    signal.throwIfAborted();
    const canvas = document.createElement("canvas");
    const ratio = screenshotPixelRatio(options.width, options.height);
    canvas.width = Math.floor(options.width * ratio);
    canvas.height = Math.floor(options.height * ratio);
    const ctx = canvas.getContext("2d");
    if (!ctx) throw new Error("canvas-unavailable");
    ctx.drawImage(image, 0, 0, canvas.width, canvas.height);
    return canvas;
  } finally {
    image.removeAttribute("src");
  }
}
