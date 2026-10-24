const MAX_CANVAS_SIDE = 16384;
const MAX_CANVAS_PIXELS = 16000000;

/** Rasterize at 2x without changing layout or exceeding mobile canvas budgets. */
export function screenshotPixelRatio(width: number, height: number): number {
  return Math.min(2, MAX_CANVAS_SIDE / width, MAX_CANVAS_SIDE / height, Math.sqrt(MAX_CANVAS_PIXELS / (width * height)));
}
