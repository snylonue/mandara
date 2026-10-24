import { expect, it } from "vitest";
import { screenshotPixelRatio } from "../src/screenshotCanvas";

it("uses 2x rasterization even on a standard-density display", () => {
  expect(screenshotPixelRatio(804, 534)).toBe(2);
  expect(screenshotPixelRatio(390, 740)).toBe(2);
});

it.each([[804, 12000], [12000, 300], [390, 12000]])("bounds oversized %s by %s exports without cropping", (width, height) => {
  const ratio = screenshotPixelRatio(width, height);
  const pixelsWide = Math.floor(width * ratio);
  const pixelsHigh = Math.floor(height * ratio);
  expect(ratio).toBeGreaterThan(0);
  expect(ratio).toBeLessThanOrEqual(2);
  expect(pixelsWide).toBeLessThanOrEqual(16384);
  expect(pixelsHigh).toBeLessThanOrEqual(16384);
  expect(pixelsWide * pixelsHigh).toBeLessThanOrEqual(16000000);
});
