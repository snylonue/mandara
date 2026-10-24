import { screenshotStep, screenshotTask } from "./screenshotTask";
import { screenshotPixelRatio } from "./screenshotCanvas";
import textStyles from "./styles/epub-content.css?raw";
import serifFontStyles from "@fontsource-variable/noto-serif-sc/index.css?inline";

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
  /** Selected reader markup with its isolated text or publication styles. */
  document?: string;
  /** Original CSS viewport; changing it changes wrapping and publication media queries. */
  viewport?: { width: number; height: number };
}

/** Snapshot the selected DOM and its publication context before a dialog takes focus. */
export function selectedEpubContent(doc: Document, selection: Selection | null): ScreenshotSelection {
  const text = selectedChapterText(doc.body, selection);
  if (!text || text.length > MAX_SCREENSHOT_TEXT) return { text };
  const range = selection!.getRangeAt(0);
  let parent = range.commonAncestorContainer;
  if (parent.nodeType !== Node.ELEMENT_NODE) parent = parent.parentNode!;
  let contents: Node = range.cloneContents();
  while (parent && parent !== doc.documentElement) {
    const wrapper = parent.cloneNode(false);
    wrapper.appendChild(contents);
    contents = wrapper;
    parent = parent.parentNode!;
  }
  const snapshot = doc.implementation.createHTMLDocument("");
  const html = doc.documentElement.cloneNode(false) as HTMLElement;
  const head = snapshot.createElement("head");
  // Only passive styles are copied; never transfer source scripts, base URLs or metadata.
  for (const style of doc.head.querySelectorAll("style")) head.append(style.cloneNode(true));
  html.append(head, contents);
  snapshot.replaceChild(html, snapshot.documentElement);
  for (const el of snapshot.querySelectorAll("script,iframe,object,embed,link,meta,base,form")) el.remove();
  for (const el of snapshot.querySelectorAll("*")) {
    for (const attr of Array.from(el.attributes)) {
      if (/^on/i.test(attr.name) || ["srcdoc", "autofocus"].includes(attr.name)) el.removeAttribute(attr.name);
    }
  }
  return { text, document: "<!doctype html>" + html.outerHTML, viewport: {
    width: doc.documentElement.clientWidth || doc.defaultView?.innerWidth || 772,
    height: doc.documentElement.clientHeight || doc.defaultView?.innerHeight || 600,
  } };
}

/** TXT uses the same DOM typography as the reader, not a second canvas typesetter. */
export function selectedTextContent(root: HTMLElement, range: Range): ScreenshotSelection {
  if (!root.contains(range.startContainer) || !root.contains(range.endContainer)) return { text: "" };
  const text = range.toString().replace(/\r\n?/g, "\n").trim();
  if (!text || text.length > MAX_SCREENSHOT_TEXT) return { text };
  let parent = range.commonAncestorContainer;
  if (parent.nodeType !== Node.ELEMENT_NODE) parent = parent.parentNode!;
  let contents: Node = range.cloneContents();
  while (parent && root.contains(parent)) {
    const wrapper = parent.cloneNode(false);
    wrapper.appendChild(contents);
    contents = wrapper;
    parent = parent.parentNode!;
  }
  const doc = root.ownerDocument.implementation.createHTMLDocument("");
  doc.body.appendChild(contents);
  doc.body.style.margin = "0";
  const inherited = getComputedStyle(root);
  for (const property of ["font-family", "font-size", "font-weight", "font-style", "line-height", "letter-spacing", "word-spacing", "color", "text-align", "text-indent", "direction", "writing-mode", "--fg", "--reader-border", "--reader-muted", "--accent", "--radius-sm", "--font-mono"]) {
    doc.documentElement.style.setProperty(property, inherited.getPropertyValue(property));
  }
  const css = doc.createElement("style");
  css.textContent = serifFontStyles + textStyles;
  doc.head.append(css);
  return { text, document: "<!doctype html>" + doc.documentElement.outerHTML, viewport: {
    width: root.getBoundingClientRect().width || root.clientWidth || 772,
    height: root.ownerDocument.defaultView?.innerHeight || 600,
  } };
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
    const width = options.viewport?.width ? Math.ceil(options.viewport.width) + PADDING * 2 : WIDTH;
    const passage = options.document
      ? await (await screenshotStep("loadRenderer", () => import("./epubScreenshot"), signal)).renderEpubScreenshot(options.document, width - PADDING * 2, MAX_HEIGHT, options.viewport?.height, signal)
      : null;
    const passageHeight = passage ? Math.ceil(passage.height * Math.min(1, (width - PADDING * 2) / passage.width)) : 0;
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
        ctx.drawImage(section.image, PADDING, y, Math.min(section.image.width, width - PADDING * 2), passageHeight);
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
