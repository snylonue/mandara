import { fireEvent } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { toSvg } from "html-to-image";
import { rasterizeScreenshot, waitForScreenshotImage } from "../src/screenshotImage";

vi.mock("html-to-image", () => ({ toSvg: vi.fn() }));
const images: HTMLImageElement[] = [];
const draw = vi.fn();
beforeEach(() => {
  vi.useFakeTimers();
  images.length = 0;
  draw.mockReset();
  vi.mocked(toSvg).mockResolvedValue("data:image/svg+xml,%3Csvg%2F%3E");
  vi.stubGlobal("Image", class {
    constructor() {
      const image = document.createElement("img");
      images.push(image);
      return image;
    }
  });
  vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockReturnValue({ drawImage: draw } as unknown as CanvasRenderingContext2D);
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
  expect(toSvg).toHaveBeenCalledWith(expect.anything(), expect.objectContaining({ fontEmbedCSS: "@font-face{}", fetchRequestInit: { signal: controller.signal } }));
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
  vi.mocked(toSvg).mockImplementationOnce(() => new Promise(() => {}));
  const controller = new AbortController();
  const rendering = rasterizeScreenshot(document.createElement("div"), { width: 390, height: 120, signal: controller.signal });
  const reason = new Error("cancelled");
  const rejected = expect(rendering).rejects.toBe(reason);
  controller.abort(reason);
  await rejected;
  expect(images).toHaveLength(0);
});
