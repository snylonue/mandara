import { fireEvent } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { snapdom, type CaptureResult, type CaptureContext, type SnapdomPlugin } from "@zumer/snapdom";
import { rasterizeScreenshot, waitForScreenshotImage } from "../src/screenshotImage";

vi.mock("@zumer/snapdom", () => ({ snapdom: vi.fn() }));
const images: HTMLImageElement[] = [];
const draw = vi.fn();
const clip = vi.fn();
const rect = vi.fn();
const fill = vi.fn();
beforeEach(() => {
  vi.useFakeTimers();
  images.length = 0;
  draw.mockReset();
  vi.mocked(snapdom).mockResolvedValue({ url: "data:image/svg+xml,%3Csvg%2F%3E", warnings: [] } as unknown as CaptureResult);
  vi.stubGlobal("Image", class {
    constructor() {
      const image = document.createElement("img");
      images.push(image);
      return image;
    }
  });
  vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockReturnValue({ drawImage: draw, beginPath: vi.fn(), rect, clip, fillRect: fill } as unknown as CanvasRenderingContext2D);
});
afterEach(() => { vi.restoreAllMocks(); vi.unstubAllGlobals(); vi.clearAllMocks(); vi.useRealTimers(); });

it.each(["reject", "pending"])("draws a loaded SVG without calling a %s decode promise", async mode => {
  const controller = new AbortController();
  const rendering = rasterizeScreenshot(document.createElement("div"), { width: 390, height: 120, signal: controller.signal, fontEmbedCSS: "@font-face{}" });
  await vi.waitFor(() => expect(images).toHaveLength(1));
  const image = images[0];
  const decode = vi.fn(() => mode === "reject" ? Promise.reject(new DOMException("Cannot decode SVG", "EncodingError")) : new Promise<void>(() => {}));
  image.decode = decode;
  fireEvent.load(image);
  await vi.runAllTimersAsync();
  const canvas = await rendering;
  expect([canvas.width, canvas.height]).toEqual([780, 240]);
  expect(draw).toHaveBeenCalledWith(image, 0, 0, 780, 240);
  expect(decode).not.toHaveBeenCalled();
  expect(image.getAttribute("src")).toBeNull();
  expect(vi.getTimerCount()).toBe(0);
  const options = vi.mocked(snapdom).mock.lastCall![1]!;
  expect(options).toMatchObject({ embedFonts: false, dpr: 1 });
  const context = { fontsCSS: "" } as CaptureContext;
  await (options.plugins![0] as SnapdomPlugin).beforeRender!(context);
  expect(context.fontsCSS).toBe("@font-face{}");
  expect(canvas.style.width).toBe("390px");
});

it("does not wait for a throttled animation frame", async () => {
  vi.spyOn(window, "requestAnimationFrame").mockReturnValue(123);
  const cancel = vi.spyOn(window, "cancelAnimationFrame");
  const rendering = rasterizeScreenshot(document.createElement("div"), { width: 390, height: 120, signal: new AbortController().signal });
  await vi.waitFor(() => expect(images).toHaveLength(1));
  fireEvent.load(images[0]);
  await vi.advanceTimersByTimeAsync(50);
  await rendering;
  expect(draw).toHaveBeenCalledOnce();
  expect(cancel).toHaveBeenCalledWith(123);
});

it("aborts an image load immediately and releases its SVG source", async () => {
  const controller = new AbortController();
  const rendering = rasterizeScreenshot(document.createElement("div"), { width: 390, height: 120, signal: controller.signal });
  await vi.waitFor(() => expect(images).toHaveLength(1));
  const reason = new Error("cancelled");
  controller.abort(reason);
  await expect(rendering).rejects.toBe(reason);
  // Check cleanup even if a browser delivers a queued load after cancellation.
  fireEvent.load(images[0]);
  expect(images[0].getAttribute("src")).toBeNull();
  expect(draw).not.toHaveBeenCalled();
  expect(vi.getTimerCount()).toBe(0);
});

it("rejects failed images rather than leaving their promise pending", async () => {
  const rendering = rasterizeScreenshot(document.createElement("div"), { width: 390, height: 120, signal: new AbortController().signal });
  const rejected = expect(rendering).rejects.toThrow("image-load");
  await vi.waitFor(() => expect(images).toHaveLength(1));
  fireEvent.error(images[0]);
  await rejected;
  expect(images[0].getAttribute("src")).toBeNull();
  expect(draw).not.toHaveBeenCalled();
});

it("recognizes cached book images without decode, including broken ones", async () => {
  const image = document.createElement("img");
  Object.defineProperties(image, { complete: { value: true }, naturalWidth: { configurable: true, value: 20 } });
  image.decode = vi.fn(() => new Promise(() => {}));
  await waitForScreenshotImage(image, new AbortController().signal);
  expect(image.decode).not.toHaveBeenCalled();
  Object.defineProperty(image, "naturalWidth", { value: 0 });
  await expect(waitForScreenshotImage(image, new AbortController().signal)).rejects.toThrow("image-load");
});

it("cancels pending SVG serialization before allocating an image", async () => {
  vi.mocked(snapdom).mockImplementationOnce(() => new Promise(() => {}));
  const controller = new AbortController();
  const rendering = rasterizeScreenshot(document.createElement("div"), { width: 390, height: 120, signal: controller.signal });
  const reason = new Error("cancelled");
  const rejected = expect(rendering).rejects.toBe(reason);
  controller.abort(reason);
  await rejected;
  expect(images).toHaveLength(0);
});

it("preserves CSS geometry while bounding actual canvas allocation", async () => {
  const rendering = rasterizeScreenshot(document.createElement("div"), { width: 772, height: 11000, signal: new AbortController().signal });
  await vi.waitFor(() => expect(images).toHaveLength(1)); fireEvent.load(images[0]); await vi.runAllTimersAsync();
  const canvas = await rendering;
  expect(canvas.width * canvas.height).toBeLessThanOrEqual(16000000);
  expect(Math.max(canvas.width, canvas.height)).toBeLessThanOrEqual(16384);
  expect(canvas.style.width).toBe("772px"); expect(canvas.style.height).toBe("11000px");
});

it("clips only pixels, composites uniform backgrounds, and does not replay publisher CSS", async () => {
  const node = document.createElement("div");
  const rendering = rasterizeScreenshot(node, { width: 90, height: 30, signal: new AbortController().signal, clip: { x: 50, y: 80, width: 90, height: 30 }, mask: [{ x: 10, y: 2, width: 40, height: 20 }], backdrop: ["rgb(20, 30, 40)"] });
  await vi.waitFor(() => expect(images).toHaveLength(1)); fireEvent.load(images[0]); await vi.runAllTimersAsync(); await rendering;
  expect(rect).toHaveBeenCalledWith(20, 4, 80, 40); expect(clip).toHaveBeenCalledOnce();
  expect(fill).toHaveBeenCalledWith(0, 0, 180, 60);
  const options = vi.mocked(snapdom).mock.lastCall![1]!;
  expect(options.clip).toEqual({ x: 50, y: 80, width: 90, height: 30 });
  for (const tag of ["head", "style", "script"]) expect(options.filter!(document.createElement(tag))).toBe(false);
  expect(options.filter!(node)).toBe(true);
});

it("rewrites prepared SVG resources only in the frozen clone", async () => {
  const node = document.createElement("div"); node.innerHTML = '<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink"><image href="/api/images/pic" xlink:href="/api/images/pic"/></svg>';
  const rendering = rasterizeScreenshot(node, { width: 20, height: 20, signal: new AbortController().signal, images: new Map([["/api/images/pic", "data:image/png;base64,AAAA"]]) });
  await vi.waitFor(() => expect(images).toHaveLength(1));
  const frozen = node.cloneNode(true) as HTMLElement;
  const plugin = vi.mocked(snapdom).mock.lastCall![1]!.plugins![0] as SnapdomPlugin;
  await plugin.afterClone!({ clone: frozen } as CaptureContext);
  expect(node.querySelector("image")!.getAttribute("href")).toBe("/api/images/pic");
  expect(frozen.querySelector("image")!.getAttribute("href")).toBe("data:image/png;base64,AAAA");
  expect(frozen.querySelector("image")!.getAttributeNS("http://www.w3.org/1999/xlink", "href")).toBe("data:image/png;base64,AAAA");
  expect(new DOMParser().parseFromString(new XMLSerializer().serializeToString(frozen), "application/xhtml+xml").querySelector("parsererror")).toBeNull();
  fireEvent.load(images[0]); await vi.runAllTimersAsync(); await rendering;
});

it("rejects resource fallbacks rather than returning a silently incomplete image", async () => {
  vi.mocked(snapdom).mockResolvedValueOnce({ warnings: [{ code: "image-fetch-failed" }] } as unknown as CaptureResult);
  await expect(rasterizeScreenshot(document.createElement("div"), { width: 20, height: 20, signal: new AbortController().signal })).rejects.toThrow("capture-image-fetch-failed");
  expect(images).toHaveLength(0); expect(draw).not.toHaveBeenCalled();
});


it("preserves measured zero-width gutters without hiding reserved native scrollbar space", async () => {
  const overlay = document.createElement("div"), reserved = document.createElement("div");
  for (const source of [overlay, reserved]) {
    source.style.cssText = "overflow:auto;border:2px solid red";
    Object.defineProperties(source, { offsetWidth: { value: 200 }, offsetHeight: { value: 100 }, clientWidth: { value: source === overlay ? 196 : 180 }, clientHeight: { value: 96 } });
  }
  const a = overlay.cloneNode(true) as HTMLElement, b = reserved.cloneNode(true) as HTMLElement;
  const rendering = rasterizeScreenshot(overlay, { width: 200, height: 100, signal: new AbortController().signal });
  await vi.waitFor(() => expect(images).toHaveLength(1));
  const plugin = vi.mocked(snapdom).mock.lastCall![1]!.plugins![0] as SnapdomPlugin;
  const context = { nodeMap: new Map([[a, overlay], [b, reserved]]), classCSS: "" } as CaptureContext;
  await plugin.beforeRender!(context);
  expect(a.style.getPropertyValue("scrollbar-width")).toBe("none");
  expect(b.style.getPropertyValue("scrollbar-width")).toBe("");
  expect(overlay.hasAttribute("data-reader-overlay-scrollbar")).toBe(false);
  fireEvent.load(images[0]); await vi.runAllTimersAsync(); await rendering;
});

it("embeds CSS URLs without changing literal generated text or the selected srcset candidate", async () => {
  const node = document.createElement("div"); node.innerHTML = '<img src="/api/images/unused" srcset="/api/images/chosen 2x">';
  const source = node.querySelector("img")!;
  Object.defineProperty(source, "currentSrc", { value: location.origin + "/api/images/chosen" });
  const rendering = rasterizeScreenshot(node, { width: 20, height: 20, signal: new AbortController().signal, images: new Map([[source.currentSrc, "data:image/png;base64,AAAA"]]) });
  await vi.waitFor(() => expect(images).toHaveLength(1));
  const clone = node.cloneNode(true) as HTMLElement;
  const url = source.currentSrc;
  const context = { clone, nodeMap: new Map([[clone.querySelector("img")!, source]]), classCSS: '.p{background:url("' + url + '")}.p::before{content:"url(' + url + ')"}' } as CaptureContext;
  await (vi.mocked(snapdom).mock.lastCall![1]!.plugins![0] as SnapdomPlugin).afterClone!(context);
  expect(context.classCSS).toContain('background:url("data:image/png;base64,AAAA")');
  expect(context.classCSS).toContain('content:"url(' + url + ')"');
  expect(clone.querySelector("img")!.getAttribute("src")).toBe("data:image/png;base64,AAAA");
  expect(clone.querySelector("img")!.hasAttribute("srcset")).toBe(false);
  expect(source.getAttribute("src")).toBe("/api/images/unused");
  expect(source.hasAttribute("srcset")).toBe(true);
  fireEvent.load(images[0]); await vi.runAllTimersAsync(); await rendering;
});
