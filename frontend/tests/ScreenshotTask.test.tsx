import { expect, it } from "vitest";
import { ScreenshotStepError, screenshotStep } from "../src/screenshotTask";

it("retains the stage and message for rejected promises and synchronous browser failures", async () => {
  const signal = new AbortController().signal;
  await expect(screenshotStep("embedFonts", () => Promise.reject(new Error("font-load")), signal))
    .rejects.toMatchObject({ name: "ScreenshotStepError", step: "embedFonts", detail: "font-load" });
  await expect(screenshotStep("encodePng", () => { throw new DOMException("Tainted canvas", "SecurityError"); }, signal))
    .rejects.toMatchObject({ step: "encodePng", detail: "Tainted canvas" });
});

it("identifies image error events and cross-realm errors instead of hiding their details", async () => {
  const signal = new AbortController().signal;
  await expect(screenshotStep("rasterize", () => Promise.reject(new Event("error")), signal))
    .rejects.toMatchObject({ step: "rasterize", detail: "error" });
  await expect(screenshotStep("loadImages", () => Promise.reject({ message: "Cannot decode image" }), signal))
    .rejects.toMatchObject({ step: "loadImages", detail: "Cannot decode image" });
});

it("preserves cancellation reasons and the original failing stage", async () => {
  const controller = new AbortController();
  const pending = screenshotStep("rasterize", () => new Promise(() => {}), controller.signal);
  const reason = new Error("screenshot-timeout");
  controller.abort(reason);
  await expect(pending).rejects.toBe(reason);
  const original = new ScreenshotStepError("embedFonts", "font-load");
  await expect(screenshotStep("rasterize", () => Promise.reject(original), new AbortController().signal))
    .rejects.toBe(original);
});
