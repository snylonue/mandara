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
