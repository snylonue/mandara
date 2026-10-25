import { screenshotStep, screenshotTask } from "./screenshotTask";
import { screenshotPixelRatio } from "./screenshotCanvas";
import { snapshotScreenshotDocument, type ScreenshotCapture } from "./screenshotSelection";

/** Extract only a selection wholly contained in the current chapter. */
export function selectedChapterText(root: Element, selection: Selection | null): string {
  if (!selection || selection.isCollapsed || selection.rangeCount !== 1) return "";
  const range = selection.getRangeAt(0);
  if (!root.contains(range.startContainer) || !root.contains(range.endContainer)) return "";
  return selection.toString().replace(/\r\n?/g, "\n").trim();
}

export const MAX_SCREENSHOT_TEXT = 6000;
const PADDING = 16;
const WIDTH = 772 + PADDING * 2;
const MAX_HEIGHT = 12000;

export interface ScreenshotStyle {
  background: string;
  foreground: string;
  muted: string;
  accent: string;
  fontFamily: string;
}

export interface ScreenshotSelection {
  text: string;
  /** Full inert reader context; only the recorded range is exported. */
  document?: string;
  capture?: ScreenshotCapture;
  /** Original CSS viewport; changing it changes wrapping and publication media queries. */
  viewport?: { width: number; height: number };
}

/** Snapshot the full publication before a dialog takes focus; crop only after layout. */
export function selectedEpubContent(doc: Document, selection: Selection | null): ScreenshotSelection {
  const text = selectedChapterText(doc.body, selection);
  if (!text || text.length > MAX_SCREENSHOT_TEXT) return { text };
  return { text, ...snapshotScreenshotDocument(doc, selection!.getRangeAt(0)) };
}

/** TXT retains the actual application CSS, ancestry and viewport, not a second typesetter. */
export function selectedTextContent(root: HTMLElement, range: Range): ScreenshotSelection {
  if (!root.contains(range.startContainer) || !root.contains(range.endContainer)) return { text: "" };
  const text = range.toString().replace(/\r\n?/g, "\n").trim();
  if (!text || text.length > MAX_SCREENSHOT_TEXT) return { text };
  return { text, ...snapshotScreenshotDocument(root.ownerDocument, range, root) };
}

export interface ScreenshotOptions extends ScreenshotSelection {
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

/** Render a selected passage and optional metadata to an opaque PNG. */
export function renderReaderScreenshot(options: ScreenshotOptions, parent?: AbortSignal): Promise<Blob> {
  return screenshotTask(async signal => {
    if (!options.text.trim() || options.text.length > MAX_SCREENSHOT_TEXT) throw new Error("selection-size");
    let width = options.viewport?.width ? Math.ceil(options.viewport.width) + PADDING * 2 : WIDTH;
    const passage = options.document
      ? await (await screenshotStep("loadRenderer", () => import("./epubScreenshot"), signal)).renderEpubScreenshot(options.document, width - PADDING * 2, MAX_HEIGHT, options.viewport?.height, signal, options.capture)
      : null;
    const passageWidth = passage ? parseFloat(passage.style.width) || width - PADDING * 2 : 0;
    if (passage && options.capture) width = Math.ceil(passageWidth) + PADDING * 2;
    const passageHeight = passage ? Math.ceil(parseFloat(passage.style.height) || passage.height * passageWidth / passage.width) : 0;
    const canvas = document.createElement("canvas");
    const ctx = canvas.getContext("2d");
    if (!ctx) throw new Error("canvas-unavailable");
    const { style } = options;
    // Keep the passage first; optional attribution is a small, shared footer.
    const metadata = [options.title, options.chapter].filter(Boolean).join(" · ");
    const sections = await Promise.all([
      { text: passage ? undefined : options.text, image: passage, size: 17, lineHeight: 34, color: style.foreground, weight: 400, gap: 0 },
      { text: metadata, size: 12, lineHeight: 18, color: style.muted, weight: 400, gap: 10 },
      { text: options.link, size: 11, lineHeight: 16, color: style.muted, weight: 400, gap: 6 },
    ].filter((section) => section.text || section.image).map(async (section) => {
      const font = section.weight + " " + section.size + "px " + style.fontFamily;
      // Canvas does not wait for unused unicode-range subsets to download.
      if (section.text) await screenshotStep("metadataFonts", () => document.fonts?.load(font, section.text!), signal);
      ctx.font = font;
      return { ...section, font, lines: section.image ? [] : wrapScreenshotText(section.text!, width - PADDING * 2, (line) => ctx.measureText(line).width) };
    }));
    const height = PADDING * 2 + sections.reduce((sum, section) => sum + section.gap + (section.image ? passageHeight : section.lines.length * section.lineHeight), 0);
    // Never silently truncate long selections or exceed mobile canvas limits.
    if (height > MAX_HEIGHT) throw new Error("selection-size");
    const ratio = screenshotPixelRatio(width, height);
    canvas.width = Math.floor(width * ratio);
    canvas.height = Math.floor(height * ratio);
    ctx.scale(ratio, ratio);
    ctx.fillStyle = style.background;
    ctx.fillRect(0, 0, width, height);
    ctx.textBaseline = "top";
    let y = PADDING;
    for (const section of sections) {
      y += section.gap;
      if (section.image) {
        ctx.drawImage(section.image, PADDING, y, passageWidth, passageHeight);
        y += passageHeight;
        continue;
      }
      ctx.font = section.font;
      ctx.fillStyle = section.color;
      for (const line of section.lines) {
        ctx.fillText(line, PADDING, y);
        y += section.lineHeight;
      }
    }
    return await screenshotStep("encodePng", () => new Promise<Blob>((resolve, reject) => canvas.toBlob((blob) => {
      if (blob) resolve(blob);
      else reject(new Error("png-unavailable"));
    }, "image/png")), signal);
  }, parent);
}
