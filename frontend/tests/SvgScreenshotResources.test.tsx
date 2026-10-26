import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { embedScreenshotSvgImages, prepareScreenshotImages } from "../src/screenshotImage";

beforeEach(() => { const computed = window.getComputedStyle.bind(window); vi.stubGlobal("getComputedStyle", (node: Element) => computed(node)); });
const xlink = "http://www.w3.org/1999/xlink";
function picture(images: string) {
  const root = document.createElement("div");
  root.innerHTML = '<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="' + xlink + '"><rect width="20" height="20" fill="red"/>' + images + '</svg>';
  return root;
}
afterEach(() => { vi.unstubAllGlobals(); vi.restoreAllMocks(); });

it("keeps self-contained SVG images and vector shapes unchanged", async () => {
  const root = picture('<image href="data:image/png;base64,aW1hZ2U="/>');
  const original = root.innerHTML;
  const fetchImage = vi.fn();
  vi.stubGlobal("fetch", fetchImage);
  await embedScreenshotSvgImages(root, new AbortController().signal);
  expect(root.innerHTML).toBe(original);
  expect(fetchImage).not.toHaveBeenCalled();
});

it.each(["https://example.com/image.svg", "/api/books/private", "javascript:bad()"])("does not fetch SVG resources outside the export image policy: %s", async reference => {
  const root = picture('<image href="' + reference + '"/>');
  const fetchImage = vi.fn();
  vi.stubGlobal("fetch", fetchImage);
  await expect(embedScreenshotSvgImages(root, new AbortController().signal)).rejects.toThrow("image-origin");
  expect(fetchImage).not.toHaveBeenCalled();
});

it("deduplicates image downloads while keeping each SVG view fragment", async () => {
  const root = picture('<image href="/api/images/figure#left"/><image xlink:href="/api/images/figure#right"/>');
  const fetchImage = vi.fn().mockResolvedValue({ ok: true, blob: async () => new Blob(["image"], { type: "image/png" }) });
  vi.stubGlobal("fetch", fetchImage);
  const signal = new AbortController().signal;
  await embedScreenshotSvgImages(root, signal);
  const images = root.querySelectorAll("image");
  expect(images[0].getAttribute("href")).toBe("data:image/png;base64,aW1hZ2U=#left");
  expect(images[1].getAttributeNS(xlink, "href")).toBe("data:image/png;base64,aW1hZ2U=#right");
  expect(fetchImage).toHaveBeenCalledExactlyOnceWith(location.origin + "/api/images/figure", { signal });
});

it("surfaces unavailable SVG image resources before rasterization", async () => {
  const root = picture('<image href="/api/images/missing"/>');
  vi.stubGlobal("fetch", vi.fn().mockResolvedValue({ ok: false }));
  await expect(embedScreenshotSvgImages(root, new AbortController().signal)).rejects.toThrow("image-load");
});

it("cancels pending SVG image requests without changing their references", async () => {
  const root = picture('<image href="/api/images/pending"/>');
  const fetchImage = vi.fn(() => new Promise(() => {}));
  vi.stubGlobal("fetch", fetchImage);
  const controller = new AbortController();
  const reason = new Error("cancelled");
  const pending = embedScreenshotSvgImages(root, controller.signal);
  const rejected = expect(pending).rejects.toBe(reason);
  controller.abort(reason);
  await rejected;
  expect(fetchImage.mock.calls[0][1].signal.aborted).toBe(true);
  expect(root.querySelector("image")!.getAttribute("href")).toBe("/api/images/pending");
});

it("rejects missing CSS backgrounds before the renderer can silently drop them", async () => {
  const node = document.createElement("p"); node.style.backgroundImage = 'url("/api/images/missing")';
  const fetchImage = vi.fn().mockResolvedValue({ ok: false }); vi.stubGlobal("fetch", fetchImage);
  await expect(prepareScreenshotImages([node], new AbortController().signal)).rejects.toThrow("image-load");
  expect(fetchImage).toHaveBeenCalledOnce();
  expect(node.style.backgroundImage).toContain("/api/images/missing");
});

it("does not fetch CSS resources outside the frame's image origin policy", async () => {
  const node = document.createElement("p"); node.style.backgroundImage = 'url("https://example.com/image.png")';
  const fetchImage = vi.fn(); vi.stubGlobal("fetch", fetchImage);
  await expect(prepareScreenshotImages([node], new AbortController().signal)).rejects.toThrow("image-origin");
  expect(fetchImage).not.toHaveBeenCalled();
});

it("does not interpret quoted generated text as a CSS image URL", async () => {
  const node = document.createElement("p"); node.style.content = '"url(https://example.com/not-an-image)"';
  expect(getComputedStyle(node).content).toContain("url(");
  const fetchImage = vi.fn(); vi.stubGlobal("fetch", fetchImage);
  expect((await prepareScreenshotImages([node], new AbortController().signal)).size).toBe(0);
  expect(fetchImage).not.toHaveBeenCalled();
});

it("cancels a stalled background fetch without modifying source attributes", async () => {
  const node = document.createElement("p"); node.style.backgroundImage = 'url("/api/images/pending")';
  vi.stubGlobal("fetch", vi.fn(() => new Promise(() => {})));
  const controller = new AbortController(), reason = new Error("cancelled");
  const pending = prepareScreenshotImages([node], controller.signal);
  const rejected = expect(pending).rejects.toBe(reason); controller.abort(reason); await rejected;
  expect(node.style.backgroundImage).toContain("/api/images/pending");
});
