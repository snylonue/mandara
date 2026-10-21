/** Extract only a selection wholly contained in the current chapter. */
export function selectedChapterText(root: Element, selection: Selection | null): string {
  if (!selection || selection.isCollapsed || selection.rangeCount !== 1) return "";
  const range = selection.getRangeAt(0);
  if (!root.contains(range.startContainer) || !root.contains(range.endContainer)) return "";
  return selection.toString().replace(/\r\n?/g, "\n").trim();
}

export const MAX_SCREENSHOT_TEXT = 6000;
const WIDTH = 900;
const PADDING = 64;
const MAX_HEIGHT = 12000;

export interface ScreenshotStyle {
  background: string;
  foreground: string;
  muted: string;
  accent: string;
  fontFamily: string;
}

export interface ScreenshotOptions {
  text: string;
  title?: string;
  chapter?: string;
  link?: string;
  style: ScreenshotStyle;
}

/** Wrap by grapheme so CJK, emoji and long URLs cannot overflow the card. */
export function wrapScreenshotText(text: string, width: number, measure: (text: string) => number): string[] {
  const segmenter = new Intl.Segmenter(undefined, { granularity: "grapheme" });
  return text.split("\n").flatMap((paragraph) => {
    const lines: string[] = [];
    let line = "";
    for (const { segment } of segmenter.segment(paragraph)) {
      if (line && measure(line + segment) > width) {
        lines.push(line);
        line = "";
      }
      line += segment;
    }
    lines.push(line);
    return lines;
  });
}

/** Render local text, not publisher HTML or remote images, to an opaque PNG. */
export async function renderReaderScreenshot(options: ScreenshotOptions): Promise<Blob> {
  if (!options.text.trim() || options.text.length > MAX_SCREENSHOT_TEXT) throw new Error("selection-size");
  await document.fonts?.ready;
  const canvas = document.createElement("canvas");
  const ctx = canvas.getContext("2d");
  if (!ctx) throw new Error("canvas-unavailable");
  const { style } = options;
  const sections = await Promise.all([
    { text: options.title, size: 30, lineHeight: 44, color: style.foreground, weight: 600 },
    { text: options.chapter, size: 22, lineHeight: 34, color: style.muted, weight: 400 },
    { text: options.text, size: 28, lineHeight: 48, color: style.foreground, weight: 400 },
    { text: options.link, size: 20, lineHeight: 30, color: style.muted, weight: 400 },
  ].filter((section) => section.text).map(async (section) => {
    const font = section.weight + " " + section.size + "px " + style.fontFamily;
    // Canvas does not wait for unused unicode-range subsets to download.
    await document.fonts?.load(font, section.text!);
    ctx.font = font;
    return { ...section, font, lines: wrapScreenshotText(section.text!, WIDTH - PADDING * 2, (line) => ctx.measureText(line).width) };
  }));
  const height = PADDING * 2 + 16 + sections.reduce((sum, section) => sum + section.lines.length * section.lineHeight + 24, 0);
  // Never silently truncate long selections or exceed mobile canvas limits.
  if (height > MAX_HEIGHT) throw new Error("selection-size");
  canvas.width = WIDTH;
  canvas.height = height;
  ctx.fillStyle = style.background;
  ctx.fillRect(0, 0, WIDTH, height);
  ctx.fillStyle = style.accent;
  ctx.fillRect(PADDING, PADDING, 48, 4);
  ctx.textBaseline = "top";
  let y = PADDING + 32;
  for (const section of sections) {
    ctx.font = section.font;
    ctx.fillStyle = section.color;
    for (const line of section.lines) {
      ctx.fillText(line, PADDING, y);
      y += section.lineHeight;
    }
    y += 24;
  }
  return new Promise((resolve, reject) => canvas.toBlob((blob) => {
    if (blob) resolve(blob);
    else reject(new Error("png-unavailable"));
  }, "image/png"));
}
